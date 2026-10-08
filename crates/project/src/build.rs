//! Builders for the standard property groups and layers.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Gradient, ShapePath, TextDoc, Value};
use effectcraft_time::Tick;

use crate::props::{GroupKind, MaskMode, ParamUi, PropGroup, Property};
use crate::{Comp, Layer, LayerId, LayerSource, LightKind, Project, Switches};

/// Id allocator borrowing the project's counter.
pub struct Ids<'a>(pub &'a mut u64);

impl Ids<'_> {
    pub fn alloc(&mut self) -> u64 {
        let v = *self.0;
        *self.0 += 1;
        v
    }
    pub fn prop(&mut self, m: &str, name: &str, v: Value) -> Property {
        Property::new(self.alloc(), m, name, v)
    }
    pub fn group(&mut self, m: &str, name: &str) -> PropGroup {
        PropGroup::new(self.alloc(), m, name)
    }
}

fn slider(min: f64, max: f64, smin: f64, smax: f64, decimals: u8) -> ParamUi {
    ParamUi::Slider { min, max, slider_min: smin, slider_max: smax, decimals }
}
fn popup(opts: &[&str]) -> ParamUi {
    ParamUi::Popup { options: opts.iter().map(|s| s.to_string()).collect() }
}

/// Layer Transform group. Values are always 3D; 2D layers show two dimensions.
pub fn transform(ids: &mut Ids, anchor: [f64; 2], position: [f64; 2]) -> PropGroup {
    let mut a = ids.prop("anchor", "Anchor Point", Value::Vec3([anchor[0], anchor[1], 0.0])).with_ui(ParamUi::Point).spatial();
    a.shown_dims = 2;
    let mut p = ids.prop("position", "Position", Value::Vec3([position[0], position[1], 0.0])).with_ui(ParamUi::Point).spatial();
    p.shown_dims = 2;
    let mut s = ids.prop("scale", "Scale", Value::Vec3([100.0, 100.0, 100.0])).with_ui(ParamUi::Percent);
    s.shown_dims = 2;
    let mut o = ids.prop("orientation", "Orientation", Value::Vec3([0.0; 3])).with_ui(ParamUi::Angle);
    o.three_d_only = true;
    let mut rx = ids.prop("rotationX", "X Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle);
    rx.three_d_only = true;
    let mut ry = ids.prop("rotationY", "Y Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle);
    ry.three_d_only = true;
    let rz = ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle);
    let op = ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0));
    ids.group("transform", "Transform").with(a).with(p).with(s).with(o).with(rx).with(ry).with(rz).with(op)
}

pub fn masks(ids: &mut Ids) -> PropGroup {
    ids.group("masks", "Masks")
}
pub fn effects(ids: &mut Ids) -> PropGroup {
    ids.group("effects", "Effects")
}

/// Mask colours cycle like AE's default mask colours.
pub const MASK_COLORS: [[u8; 3]; 8] = [
    [0xb1, 0xb4, 0x4f],
    [0x4f, 0x8c, 0xc9],
    [0xc9, 0x4f, 0x8c],
    [0x4f, 0xc9, 0x7a],
    [0xc9, 0x8c, 0x4f],
    [0x8c, 0x4f, 0xc9],
    [0x4f, 0xc9, 0xc9],
    [0xc9, 0x4f, 0x4f],
];

pub fn mask(ids: &mut Ids, name: &str, path: ShapePath, mode: MaskMode, color: [u8; 3]) -> PropGroup {
    let mut g = ids.group("mask", name);
    g.kind = GroupKind::Mask {
        mode,
        inverted: false,
        color,
        locked: false,
        motion_blur: Default::default(),
        feather_falloff: Default::default(),
        roto_bezier: false,
    };
    g.with(ids.prop("path", "Mask Path", Value::Path(path)).with_ui(ParamUi::Path))
        .with(ids.prop("feather", "Mask Feather", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Pixels))
        .with(ids.prop("opacity", "Mask Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
        .with(ids.prop("expansion", "Mask Expansion", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
}

/// Inter-Character Blending modes (blend mode names, see `BlendMode::from_name`).
pub const INTER_CHAR_BLEND_MODES: &[&str] = &[
    "Normal",
    "Darken",
    "Multiply",
    "Color Burn",
    "Linear Burn",
    "Darker Color",
    "Add",
    "Lighten",
    "Screen",
    "Color Dodge",
    "Lighter Color",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Linear Light",
    "Vivid Light",
    "Pin Light",
    "Hard Mix",
    "Difference",
    "Exclusion",
    "Subtract",
    "Divide",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

/// A non-keyframable header-like property (no stopwatch, as in AE's selector popups).
fn fixed(mut p: Property) -> Property {
    p.static_only = true;
    p
}

/// Path Options group (Path popup: 0 = None, k = mask k).
pub fn path_options(ids: &mut Ids) -> PropGroup {
    let mut path = ids.prop("path", "Path", Value::Enum(0)).with_ui(popup(&["None"]));
    path.static_only = true;
    ids.group("pathOptions", "Path Options")
        .with(path)
        .with(ids.prop("reversePath", "Reverse Path", Value::Bool(false)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("perpendicular", "Perpendicular To Path", Value::Bool(true)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("forceAlignment", "Force Alignment", Value::Bool(false)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("firstMargin", "First Margin", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
        .with(ids.prop("lastMargin", "Last Margin", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
}

/// More Options group.
pub fn more_options(ids: &mut Ids) -> PropGroup {
    ids.group("moreOptions", "More Options")
        .with(fixed(ids.prop("anchorGrouping", "Anchor Point Grouping", Value::Enum(0)).with_ui(popup(&["Character", "Word", "Line", "All"]))))
        .with(ids.prop("groupingAlignment", "Grouping Alignment", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Percent))
        .with(fixed(ids.prop("fillStroke", "Fill & Stroke", Value::Enum(0)).with_ui(popup(&[
            "Per Character Palette",
            "All Fills Over All Strokes",
            "All Strokes Over All Fills",
        ]))))
        .with(fixed(ids.prop("interCharBlend", "Inter-Character Blending", Value::Enum(0)).with_ui(popup(INTER_CHAR_BLEND_MODES))))
}

pub fn text(ids: &mut Ids, doc: TextDoc) -> PropGroup {
    let mut st = ids.prop("sourceText", "Source Text", Value::Text(Box::new(doc))).with_ui(ParamUi::Text);
    st.hold_only = true;
    // Animation ▸ Animate Text ▸ Enable Per-character 3D (layer-level flag, not keyframable).
    let mut pc3 = ids.prop("perChar3d", "Per-character 3D", Value::Bool(false)).with_ui(ParamUi::Hidden);
    pc3.static_only = true;
    let path_opts = path_options(ids);
    let more = more_options(ids);
    ids.group("text", "Text").with(st).with(pc3).with(path_opts).with(more).with(ids.group("animators", "Animators"))
}

/// A text animator with one range selector and the given properties.
pub fn text_animator(ids: &mut Ids, name: &str, props: Vec<Property>) -> PropGroup {
    let mut g = ids.group("animator", name);
    g.kind = GroupKind::Indexed;
    let mut sels = ids.group("selectors", "Selectors");
    sels.children.push(range_selector(ids, "Range Selector 1").into());
    let mut pg = ids.group("properties", "Properties");
    for p in props {
        pg.children.push(p.into());
    }
    g.with(sels).with(pg)
}

const BASED_ON: &[&str] = &["Characters", "Characters Excluding Spaces", "Words", "Lines"];
const SEL_MODES: &[&str] = &["Add", "Subtract", "Intersect", "Min", "Max", "Difference"];

pub fn range_selector(ids: &mut Ids, name: &str) -> PropGroup {
    let mut g = ids.group("rangeSelector", name);
    g.kind = GroupKind::Indexed;
    let adv = ids
        .group("advanced", "Advanced")
        .with(fixed(ids.prop("units", "Units", Value::Enum(0)).with_ui(popup(&["Percentage", "Index"]))))
        .with(fixed(ids.prop("basedOn", "Based On", Value::Enum(0)).with_ui(popup(BASED_ON))))
        .with(fixed(ids.prop("mode", "Mode", Value::Enum(0)).with_ui(popup(SEL_MODES))))
        .with(ids.prop("amount", "Amount", Value::Scalar(100.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 0)))
        .with(fixed(ids.prop("shape", "Shape", Value::Enum(0)).with_ui(popup(&["Square", "Ramp Up", "Ramp Down", "Triangle", "Round", "Smooth"]))))
        .with(ids.prop("smoothness", "Smoothness", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
        .with(ids.prop("easeHigh", "Ease High", Value::Scalar(0.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 0)))
        .with(ids.prop("easeLow", "Ease Low", Value::Scalar(0.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 0)))
        .with(fixed(ids.prop("randomize", "Randomize Order", Value::Bool(false)).with_ui(ParamUi::Checkbox)))
        .with(ids.prop("randomSeed", "Random Seed", Value::Scalar(0.0)));
    g.with(ids.prop("start", "Start", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("end", "End", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("offset", "Offset", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(adv)
}

/// Wiggly Selector (Mode defaults to Intersect, so it varies the selectors above it).
pub fn wiggly_selector(ids: &mut Ids, name: &str) -> PropGroup {
    let mut g = ids.group("wigglySelector", name);
    g.kind = GroupKind::Indexed;
    g.with(fixed(ids.prop("mode", "Mode", Value::Enum(2)).with_ui(popup(SEL_MODES))))
        .with(ids.prop("maxAmount", "Max Amount", Value::Scalar(100.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 0)))
        .with(ids.prop("minAmount", "Min Amount", Value::Scalar(-100.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 0)))
        .with(fixed(ids.prop("basedOn", "Based On", Value::Enum(0)).with_ui(popup(BASED_ON))))
        .with(ids.prop("wigglesPerSecond", "Wiggles/Second", Value::Scalar(2.0)).with_ui(slider(0.0, 1000.0, 0.0, 10.0, 1)))
        .with(ids.prop("correlation", "Correlation", Value::Scalar(50.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
        .with(ids.prop("temporalPhase", "Temporal Phase", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("spatialPhase", "Spatial Phase", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("lockDimensions", "Lock Dimensions", Value::Bool(false)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("randomSeed", "Random Seed", Value::Scalar(0.0)))
}

/// Default Expression Selector amount: a ramp across the units.
pub const EXPRESSION_SELECTOR_DEFAULT: &str = "selectorValue * textIndex / textTotal";

/// Expression Selector: Amount is computed per unit by its expression (`textIndex`,
/// `textTotal`, `selectorValue` = the selection of the selectors above it) and replaces that
/// selection (there is no Mode: the expression combines with `selectorValue` itself).
pub fn expression_selector(ids: &mut Ids, name: &str) -> PropGroup {
    let mut g = ids.group("expressionSelector", name);
    g.kind = GroupKind::Indexed;
    let mut amount = ids.prop("amount", "Amount", Value::Vec3([100.0, 100.0, 100.0])).with_ui(ParamUi::Percent);
    amount.expr = Some(crate::props::Expression { text: EXPRESSION_SELECTOR_DEFAULT.into(), enabled: true });
    g.with(fixed(ids.prop("basedOn", "Based On", Value::Enum(0)).with_ui(popup(BASED_ON)))).with(amount)
}

/// A selector by kind (`range`, `wiggly`, `expression`).
pub fn text_selector(ids: &mut Ids, kind: &str, name: &str) -> Option<PropGroup> {
    Some(match kind {
        "range" => range_selector(ids, name),
        "wiggly" => wiggly_selector(ids, name),
        "expression" => expression_selector(ids, name),
        _ => return None,
    })
}

/// Animation ▸ Animate Text entries: (`layer.addTextAnimator` property kind, menu label);
/// `"-"` is a separator.
pub const TEXT_ANIMATOR_KINDS: &[(&str, &str)] = &[
    ("anchor", "Anchor Point"),
    ("position", "Position"),
    ("scale", "Scale"),
    ("skew", "Skew"),
    ("rotation", "Rotation"),
    ("opacity", "Opacity"),
    ("transformAll", "All Transform Properties"),
    ("-", "-"),
    ("lineAnchor", "Line Anchor"),
    ("lineSpacing", "Line Spacing"),
    ("characterOffset", "Character Offset"),
    ("characterValue", "Character Value"),
    ("blur", "Blur"),
    ("-", "-"),
    ("fillColor", "Fill Color: RGB"),
    ("fillHue", "Fill Color: Hue"),
    ("fillSaturation", "Fill Color: Saturation"),
    ("fillBrightness", "Fill Color: Brightness"),
    ("fillOpacity", "Fill Color: Opacity"),
    ("strokeColor", "Stroke Color: RGB"),
    ("strokeHue", "Stroke Color: Hue"),
    ("strokeSaturation", "Stroke Color: Saturation"),
    ("strokeBrightness", "Stroke Color: Brightness"),
    ("strokeOpacity", "Stroke Color: Opacity"),
    ("strokeWidth", "Stroke Width"),
    ("tracking", "Tracking"),
];

fn vec3_prop(ids: &mut Ids, m: &str, name: &str, v: [f64; 3], ui: ParamUi, three_d: bool) -> Property {
    let mut p = ids.prop(m, name, Value::Vec3(v)).with_ui(ui);
    p.shown_dims = if three_d { 3 } else { 2 };
    p
}

fn pct_slider(ids: &mut Ids, m: &str, name: &str, v: f64, min: f64) -> Property {
    ids.prop(m, name, Value::Scalar(v)).with_ui(slider(min, 100.0, min, 100.0, 0))
}

/// The properties one Animate / Add ▸ Property entry adds (AE adds companions: Skew Axis with
/// Skew, Tracking Type with Tracking, Character Alignment / Range with Character Offset, X / Y
/// Rotation with Rotation when per-character 3D is on). `three_d` shows Z components.
pub fn text_anim_props(ids: &mut Ids, kind: &str, three_d: bool) -> Vec<Property> {
    match kind {
        "anchor" => vec![vec3_prop(ids, "anchor", "Anchor Point", [0.0; 3], ParamUi::Point, three_d)],
        "position" => vec![vec3_prop(ids, "position", "Position", [0.0; 3], ParamUi::Point, three_d)],
        "scale" => vec![vec3_prop(ids, "scale", "Scale", [100.0; 3], ParamUi::Percent, three_d)],
        "skew" => vec![
            ids.prop("skew", "Skew", Value::Scalar(0.0)).with_ui(slider(-85.0, 85.0, -70.0, 70.0, 1)),
            ids.prop("skewAxis", "Skew Axis", Value::Scalar(0.0)).with_ui(ParamUi::Angle),
        ],
        "rotation" => {
            if three_d {
                vec![
                    ids.prop("rotationX", "X Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle),
                    ids.prop("rotationY", "Y Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle),
                    ids.prop("rotation", "Z Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle),
                ]
            } else {
                vec![ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle)]
            }
        }
        "opacity" => vec![pct_slider(ids, "opacity", "Opacity", 100.0, 0.0)],
        "transformAll" => ["anchor", "position", "scale", "skew", "rotation", "opacity"].iter().flat_map(|k| text_anim_props(ids, k, three_d)).collect(),
        "lineAnchor" => vec![ids.prop("lineAnchor", "Line Anchor", Value::Scalar(0.0)).with_ui(ParamUi::Percent)],
        "lineSpacing" => vec![ids.prop("lineSpacing", "Line Spacing", Value::Vec2([0.0, 0.0]))],
        "characterOffset" => vec![char_alignment(ids), char_range(ids), ids.prop("characterOffset", "Character Offset", Value::Scalar(0.0))],
        "characterValue" => vec![char_range(ids), ids.prop("characterValue", "Character Value", Value::Scalar(0.0))],
        "blur" => vec![ids.prop("blur", "Blur", Value::Vec2([0.0, 0.0]))],
        "fillColor" => vec![ids.prop("fillColor", "Fill Color", Value::Color([1.0, 0.0, 0.0, 1.0])).with_ui(ParamUi::Color)],
        "fillHue" => vec![ids.prop("fillHue", "Fill Hue", Value::Scalar(0.0)).with_ui(ParamUi::Angle)],
        "fillSaturation" => vec![pct_slider(ids, "fillSaturation", "Fill Saturation", 0.0, -100.0)],
        "fillBrightness" => vec![pct_slider(ids, "fillBrightness", "Fill Brightness", 0.0, -100.0)],
        "fillOpacity" => vec![pct_slider(ids, "fillOpacity", "Fill Opacity", 100.0, 0.0)],
        "strokeColor" => vec![ids.prop("strokeColor", "Stroke Color", Value::Color([1.0, 0.0, 0.0, 1.0])).with_ui(ParamUi::Color)],
        "strokeHue" => vec![ids.prop("strokeHue", "Stroke Hue", Value::Scalar(0.0)).with_ui(ParamUi::Angle)],
        "strokeSaturation" => vec![pct_slider(ids, "strokeSaturation", "Stroke Saturation", 0.0, -100.0)],
        "strokeBrightness" => vec![pct_slider(ids, "strokeBrightness", "Stroke Brightness", 0.0, -100.0)],
        "strokeOpacity" => vec![pct_slider(ids, "strokeOpacity", "Stroke Opacity", 100.0, 0.0)],
        "strokeWidth" => vec![ids.prop("strokeWidth", "Stroke Width", Value::Scalar(0.0)).with_ui(ParamUi::Pixels)],
        "tracking" => {
            let tt = fixed(ids.prop("trackingType", "Tracking Type", Value::Enum(0)).with_ui(popup(&["Before & After", "Before", "After"])));
            vec![tt, ids.prop("tracking", "Tracking Amount", Value::Scalar(0.0))]
        }
        _ => vec![],
    }
}

fn char_alignment(ids: &mut Ids) -> Property {
    fixed(ids.prop("characterAlignment", "Character Alignment", Value::Enum(1)).with_ui(popup(&["Left or Top", "Center", "Right or Bottom", "Adjust Kerning"])))
}

fn char_range(ids: &mut Ids) -> Property {
    fixed(ids.prop("characterRange", "Character Range", Value::Enum(0)).with_ui(popup(&["Preserve Case & Digits", "Full Unicode"])))
}

/// The main property of an animator kind (the last of [`text_anim_props`]; kept for callers
/// that add one property).
pub fn text_anim_prop(ids: &mut Ids, kind: &str) -> Option<Property> {
    let mut v = text_anim_props(ids, kind, false);
    v.pop()
}

// ---------------------------------------------------------------- shape layer contents

pub fn contents(ids: &mut Ids) -> PropGroup {
    ids.group("contents", "Contents")
}

/// Shape group transform (2D).
pub fn shape_transform(ids: &mut Ids) -> PropGroup {
    ids.group("transform", "Transform")
        .with(ids.prop("anchor", "Anchor Point", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Point))
        .with(ids.prop("position", "Position", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Point))
        .with(ids.prop("scale", "Scale", Value::Vec2([100.0, 100.0])).with_ui(ParamUi::Percent))
        .with(ids.prop("skew", "Skew", Value::Scalar(0.0)).with_ui(slider(-85.0, 85.0, -85.0, 85.0, 1)))
        .with(ids.prop("skewAxis", "Skew Axis", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
}

fn indexed(mut g: PropGroup) -> PropGroup {
    g.kind = GroupKind::Indexed;
    g
}

pub fn shape_group(ids: &mut Ids, name: &str, items: Vec<PropGroup>) -> PropGroup {
    let mut c = ids.group("contents", "Contents");
    for i in items {
        c.children.push(i.into());
    }
    let mut g = indexed(ids.group("group", name));
    g.children.push(ids.prop("blend", "Blend Mode", Value::Enum(0)).with_ui(ParamUi::Hidden).into());
    g.with(c).with(shape_transform(ids))
}

pub fn shape_rect(ids: &mut Ids, size: [f64; 2], position: [f64; 2], roundness: f64) -> PropGroup {
    indexed(ids.group("rect", "Rectangle Path 1"))
        .with(ids.prop("direction", "Path Direction", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("size", "Size", Value::Vec2(size)).with_ui(ParamUi::Pixels))
        .with(ids.prop("position", "Position", Value::Vec2(position)).with_ui(ParamUi::Point))
        .with(ids.prop("roundness", "Roundness", Value::Scalar(roundness)).with_ui(ParamUi::Pixels))
}

pub fn shape_ellipse(ids: &mut Ids, size: [f64; 2], position: [f64; 2]) -> PropGroup {
    indexed(ids.group("ellipse", "Ellipse Path 1"))
        .with(ids.prop("direction", "Path Direction", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("size", "Size", Value::Vec2(size)).with_ui(ParamUi::Pixels))
        .with(ids.prop("position", "Position", Value::Vec2(position)).with_ui(ParamUi::Point))
}

/// Polystar: `star = false` makes a polygon.
pub fn shape_star(ids: &mut Ids, star: bool, points: f64, position: [f64; 2], outer: f64, inner: f64) -> PropGroup {
    let mut g = indexed(ids.group("star", "Polystar Path 1"))
        .with(ids.prop("direction", "Path Direction", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("type", "Type", Value::Enum(if star { 0 } else { 1 })).with_ui(popup(&["Star", "Polygon"])))
        .with(ids.prop("points", "Points", Value::Scalar(points)).with_ui(slider(3.0, 100.0, 3.0, 20.0, 1)))
        .with(ids.prop("position", "Position", Value::Vec2(position)).with_ui(ParamUi::Point))
        .with(ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle));
    if star {
        g = g.with(ids.prop("innerRadius", "Inner Radius", Value::Scalar(inner)).with_ui(ParamUi::Pixels));
    }
    g = g.with(ids.prop("outerRadius", "Outer Radius", Value::Scalar(outer)).with_ui(ParamUi::Pixels));
    if star {
        g = g.with(ids.prop("innerRoundness", "Inner Roundness", Value::Scalar(0.0)).with_ui(ParamUi::Percent));
    }
    g.with(ids.prop("outerRoundness", "Outer Roundness", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
}

pub fn shape_path(ids: &mut Ids, path: ShapePath) -> PropGroup {
    indexed(ids.group("path", "Path 1"))
        .with(ids.prop("direction", "Path Direction", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("path", "Path", Value::Path(path)).with_ui(ParamUi::Path))
}

pub fn shape_fill(ids: &mut Ids, color: [f64; 4]) -> PropGroup {
    indexed(ids.group("fill", "Fill 1"))
        .with(ids.prop("blend", "Blend Mode", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("composite", "Composite", Value::Enum(0)).with_ui(popup(&["Below Previous in Same Group", "Above Previous in Same Group"])))
        .with(ids.prop("rule", "Fill Rule", Value::Enum(0)).with_ui(popup(&["Non-Zero Winding", "Even-Odd"])))
        .with(ids.prop("color", "Color", Value::Color(color)).with_ui(ParamUi::Color))
        .with(ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
}

pub fn shape_stroke(ids: &mut Ids, color: [f64; 4], width: f64) -> PropGroup {
    indexed(ids.group("stroke", "Stroke 1"))
        .with(ids.prop("blend", "Blend Mode", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("composite", "Composite", Value::Enum(0)).with_ui(popup(&["Below Previous in Same Group", "Above Previous in Same Group"])))
        .with(ids.prop("color", "Color", Value::Color(color)).with_ui(ParamUi::Color))
        .with(ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
        .with(ids.prop("width", "Stroke Width", Value::Scalar(width)).with_ui(ParamUi::Pixels))
        .with(ids.prop("cap", "Line Cap", Value::Enum(0)).with_ui(popup(&["Butt Cap", "Round Cap", "Projecting Cap"])))
        .with(ids.prop("join", "Line Join", Value::Enum(0)).with_ui(popup(&["Miter Join", "Round Join", "Bevel Join"])))
        .with(ids.prop("miter", "Miter Limit", Value::Scalar(4.0)))
        .with(
            ids.group("dashes", "Dashes")
                .with(ids.prop("dash", "Dash", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
                .with(ids.prop("gap", "Gap", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
                .with(ids.prop("offset", "Offset", Value::Scalar(0.0)).with_ui(ParamUi::Pixels)),
        )
        .with(stroke_taper(ids))
        .with(stroke_wave(ids))
}

/// Stroke ▸ Taper (After Effects 17.1+): width ramps at the start and end of each path.
pub fn stroke_taper(ids: &mut Ids) -> PropGroup {
    ids.group("taper", "Taper")
        .with(ids.prop("units", "Length Units", Value::Enum(0)).with_ui(popup(&["Pixels", "Percent"])))
        .with(ids.prop("startLength", "Start Length", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
        .with(ids.prop("endLength", "End Length", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
        .with(ids.prop("startWidth", "Start Width", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("endWidth", "End Width", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("startEase", "Start Ease", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("endEase", "End Ease", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
}

/// Stroke ▸ Wave: the width oscillates along the path.
pub fn stroke_wave(ids: &mut Ids) -> PropGroup {
    ids.group("wave", "Wave")
        .with(ids.prop("amount", "Amount", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("units", "Units", Value::Enum(0)).with_ui(popup(&["Pixels", "Cycles"])))
        .with(ids.prop("wavelength", "Wavelength", Value::Scalar(20.0)).with_ui(ParamUi::Pixels))
        .with(ids.prop("cycles", "Cycles", Value::Scalar(10.0)).with_ui(slider(0.0, 1000.0, 0.0, 50.0, 1)))
        .with(ids.prop("phase", "Phase", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
}

/// Match ids of the optional Dash/Gap pairs After Effects adds with the Dashes "+" button.
pub const EXTRA_DASHES: [(&str, &str); 2] = [("dash2", "gap2"), ("dash3", "gap3")];

pub fn shape_gradient_fill(ids: &mut Ids, radial: bool, start: [f64; 2], end: [f64; 2], g: Gradient) -> PropGroup {
    indexed(ids.group("gfill", "Gradient Fill 1"))
        .with(ids.prop("blend", "Blend Mode", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("rule", "Fill Rule", Value::Enum(0)).with_ui(popup(&["Non-Zero Winding", "Even-Odd"])))
        .with(ids.prop("type", "Type", Value::Enum(radial as u32)).with_ui(popup(&["Linear", "Radial"])))
        .with(ids.prop("start", "Start Point", Value::Vec2(start)).with_ui(ParamUi::Point))
        .with(ids.prop("end", "End Point", Value::Vec2(end)).with_ui(ParamUi::Point))
        .with(ids.prop("highlightLength", "Highlight Length", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("highlightAngle", "Highlight Angle", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("colors", "Colors", Value::Gradient(g)).with_ui(ParamUi::Gradient))
        .with(ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
}

/// Gradient Stroke: a stroke painted with a linear or radial gradient.
pub fn shape_gradient_stroke(ids: &mut Ids, radial: bool, start: [f64; 2], end: [f64; 2], g: Gradient, width: f64) -> PropGroup {
    indexed(ids.group("gstroke", "Gradient Stroke 1"))
        .with(ids.prop("blend", "Blend Mode", Value::Enum(0)).with_ui(ParamUi::Hidden))
        .with(ids.prop("composite", "Composite", Value::Enum(0)).with_ui(popup(&["Below Previous in Same Group", "Above Previous in Same Group"])))
        .with(ids.prop("type", "Type", Value::Enum(radial as u32)).with_ui(popup(&["Linear", "Radial"])))
        .with(ids.prop("start", "Start Point", Value::Vec2(start)).with_ui(ParamUi::Point))
        .with(ids.prop("end", "End Point", Value::Vec2(end)).with_ui(ParamUi::Point))
        .with(ids.prop("highlightLength", "Highlight Length", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("highlightAngle", "Highlight Angle", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("colors", "Colors", Value::Gradient(g)).with_ui(ParamUi::Gradient))
        .with(ids.prop("opacity", "Opacity", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)))
        .with(ids.prop("width", "Stroke Width", Value::Scalar(width)).with_ui(ParamUi::Pixels))
        .with(ids.prop("cap", "Line Cap", Value::Enum(0)).with_ui(popup(&["Butt Cap", "Round Cap", "Projecting Cap"])))
        .with(ids.prop("join", "Line Join", Value::Enum(0)).with_ui(popup(&["Miter Join", "Round Join", "Bevel Join"])))
        .with(ids.prop("miter", "Miter Limit", Value::Scalar(4.0)))
        .with(
            ids.group("dashes", "Dashes")
                .with(ids.prop("dash", "Dash", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
                .with(ids.prop("gap", "Gap", Value::Scalar(0.0)).with_ui(ParamUi::Pixels))
                .with(ids.prop("offset", "Offset", Value::Scalar(0.0)).with_ui(ParamUi::Pixels)),
        )
        .with(stroke_taper(ids))
        .with(stroke_wave(ids))
}

pub fn shape_trim(ids: &mut Ids, start: f64, end: f64, offset: f64) -> PropGroup {
    indexed(ids.group("trim", "Trim Paths 1"))
        .with(ids.prop("start", "Start", Value::Scalar(start)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("end", "End", Value::Scalar(end)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("offset", "Offset", Value::Scalar(offset)).with_ui(ParamUi::Angle))
        .with(ids.prop("mode", "Trim Multiple Shapes", Value::Enum(0)).with_ui(popup(&["Simultaneously", "Individually"])))
}

pub fn shape_repeater(ids: &mut Ids, copies: f64, offset_pos: [f64; 2]) -> PropGroup {
    let tr = ids
        .group("transform", "Transform")
        .with(ids.prop("anchor", "Anchor Point", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Point))
        .with(ids.prop("position", "Position", Value::Vec2(offset_pos)).with_ui(ParamUi::Point))
        .with(ids.prop("scale", "Scale", Value::Vec2([100.0, 100.0])).with_ui(ParamUi::Percent))
        .with(ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("startOpacity", "Start Opacity", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("endOpacity", "End Opacity", Value::Scalar(100.0)).with_ui(ParamUi::Percent));
    indexed(ids.group("repeater", "Repeater 1"))
        .with(ids.prop("copies", "Copies", Value::Scalar(copies)))
        .with(ids.prop("offset", "Offset", Value::Scalar(0.0)))
        .with(ids.prop("composite", "Composite", Value::Enum(0)).with_ui(popup(&["Below", "Above"])))
        .with(tr)
}

pub fn shape_simple_op(ids: &mut Ids, kind: &str) -> Option<PropGroup> {
    let g = match kind {
        "round" => indexed(ids.group("round", "Round Corners 1")).with(ids.prop("radius", "Radius", Value::Scalar(10.0)).with_ui(ParamUi::Pixels)),
        "offset" => indexed(ids.group("offset", "Offset Paths 1"))
            .with(ids.prop("amount", "Amount", Value::Scalar(10.0)).with_ui(ParamUi::Pixels))
            .with(ids.prop("join", "Line Join", Value::Enum(0)).with_ui(popup(&["Miter Join", "Round Join", "Bevel Join"])))
            .with(ids.prop("miter", "Miter Limit", Value::Scalar(4.0)))
            .with(ids.prop("copies", "Copies", Value::Scalar(1.0)))
            .with(ids.prop("copyOffset", "Copy Offset", Value::Scalar(0.0))),
        "pucker" => indexed(ids.group("pucker", "Pucker & Bloat 1"))
            .with(ids.prop("amount", "Amount", Value::Scalar(0.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 1))),
        "twist" => indexed(ids.group("twist", "Twist 1"))
            .with(ids.prop("angle", "Angle", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
            .with(ids.prop("center", "Center", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Point)),
        "zigzag" => indexed(ids.group("zigzag", "Zig Zag 1"))
            .with(ids.prop("size", "Size", Value::Scalar(10.0)).with_ui(ParamUi::Pixels))
            .with(ids.prop("ridges", "Ridges per segment", Value::Scalar(5.0)))
            .with(ids.prop("points", "Points", Value::Enum(0)).with_ui(popup(&["Corner", "Smooth"]))),
        "wiggle" => indexed(ids.group("wiggle", "Wiggle Paths 1"))
            .with(ids.prop("size", "Size", Value::Scalar(10.0)).with_ui(ParamUi::Pixels))
            .with(ids.prop("detail", "Detail", Value::Scalar(10.0)))
            .with(ids.prop("points", "Points", Value::Enum(1)).with_ui(popup(&["Corner", "Smooth"])))
            .with(ids.prop("speed", "Wiggles/Second", Value::Scalar(2.0)))
            .with(ids.prop("correlation", "Correlation", Value::Scalar(50.0)).with_ui(ParamUi::Percent))
            .with(ids.prop("phase", "Temporal Phase", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
            .with(ids.prop("seed", "Random Seed", Value::Scalar(0.0))),
        "merge" => indexed(ids.group("merge", "Merge Paths 1")).with(ids.prop("mode", "Mode", Value::Enum(0)).with_ui(popup(&[
            "Merge",
            "Add",
            "Subtract",
            "Intersect",
            "Exclude Intersections",
        ]))),
        _ => return None,
    };
    Some(g)
}

// ---------------------------------------------------------------- camera / light / 3D

pub fn camera_options(ids: &mut Ids, zoom: f64) -> PropGroup {
    camera_options_for(ids, zoom, zoom * 36.0 / 50.0)
}

/// Camera Options for a comp `comp_w` wide (aperture from the 50 mm preset: 25.3 mm at 1920 px).
pub fn camera_options_for(ids: &mut Ids, zoom: f64, comp_w: f64) -> PropGroup {
    let dof = ids.prop("dof", "Depth of Field", Value::Bool(false)).with_ui(ParamUi::Checkbox);
    ids.group("cameraOptions", "Camera Options")
        .with(ids.prop("zoom", "Zoom", Value::Scalar(zoom)).with_ui(ParamUi::Pixels))
        .with(dof)
        .with(ids.prop("focusDistance", "Focus Distance", Value::Scalar(zoom)).with_ui(ParamUi::Pixels))
        .with(ids.prop("aperture", "Aperture", Value::Scalar(25.3 * comp_w / 1920.0)).with_ui(ParamUi::Pixels))
        .with(ids.prop("blurLevel", "Blur Level", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("irisShape", "Iris Shape", Value::Enum(0)).with_ui(popup(&[
            "Fast Rectangle",
            "Triangle",
            "Square",
            "Pentagon",
            "Hexagon",
            "Heptagon",
            "Octagon",
            "Nonagon",
            "Decagon",
        ])))
        .with(ids.prop("irisRotation", "Iris Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
        .with(ids.prop("irisRoundness", "Iris Roundness", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("irisAspectRatio", "Iris Aspect Ratio", Value::Scalar(1.0)).with_ui(slider(0.01, 100.0, 0.01, 4.0, 2)))
        .with(ids.prop("irisDiffractionFringe", "Iris Diffraction Fringe", Value::Scalar(0.0)).with_ui(slider(0.0, 1000.0, 0.0, 100.0, 1)))
        .with(ids.prop("highlightGain", "Highlight Gain", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("highlightThreshold", "Highlight Threshold", Value::Scalar(255.0)).with_ui(slider(0.0, 255.0, 0.0, 255.0, 0)))
        .with(ids.prop("highlightSaturation", "Highlight Saturation", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
}

pub fn light_options(ids: &mut Ids, kind: LightKind) -> PropGroup {
    let mut g = ids
        .group("lightOptions", "Light Options")
        .with(ids.prop("intensity", "Intensity", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("color", "Color", Value::Color([1.0, 1.0, 1.0, 1.0])).with_ui(ParamUi::Color));
    if kind == LightKind::Spot {
        g = g
            .with(ids.prop("coneAngle", "Cone Angle", Value::Scalar(90.0)).with_ui(ParamUi::Angle))
            .with(ids.prop("coneFeather", "Cone Feather", Value::Scalar(50.0)).with_ui(ParamUi::Percent));
    }
    if kind == LightKind::Environment {
        // The environment image: a layer (footage, comp…) mapped as an equirectangular panorama;
        // none = the comp's Environment Layer.
        return g
            .with(ids.prop("source", "Source", Value::Layer(None)).with_ui(ParamUi::Layer))
            .with(ids.prop("rotation", "Environment Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle));
    }
    if kind != LightKind::Ambient {
        g = g
            .with(ids.prop("falloff", "Falloff", Value::Enum(0)).with_ui(popup(&["None", "Smooth", "Inverse Square Clamped"])))
            .with(ids.prop("radius", "Radius", Value::Scalar(500.0)).with_ui(ParamUi::Pixels))
            .with(ids.prop("falloffDistance", "Falloff Distance", Value::Scalar(500.0)).with_ui(ParamUi::Pixels))
            .with(ids.prop("castsShadows", "Casts Shadows", Value::Bool(false)).with_ui(ParamUi::Checkbox))
            .with(ids.prop("shadowDarkness", "Shadow Darkness", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
            .with(ids.prop("shadowDiffusion", "Shadow Diffusion", Value::Scalar(0.0)).with_ui(ParamUi::Pixels));
    }
    g
}

pub fn material_options(ids: &mut Ids) -> PropGroup {
    ids.group("materialOptions", "Material Options")
        .with(ids.prop("castsShadows", "Casts Shadows", Value::Enum(0)).with_ui(popup(&["Off", "On", "Only"])))
        .with(ids.prop("lightTransmission", "Light Transmission", Value::Scalar(0.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("acceptsShadows", "Accepts Shadows", Value::Enum(1)).with_ui(popup(&["Off", "On", "Only"])))
        .with(ids.prop("acceptsLights", "Accepts Lights", Value::Bool(true)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("ambient", "Ambient", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("diffuse", "Diffuse", Value::Scalar(50.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("specularIntensity", "Specular Intensity", Value::Scalar(50.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("specularShininess", "Specular Shininess", Value::Scalar(5.0)).with_ui(ParamUi::Percent))
        .with(ids.prop("metal", "Metal", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
}

/// Material Options of a 3D model or primitive layer (Advanced 3D): shadows and lights, plus
/// the primitive's own metallic-roughness material (models keep their file's materials).
pub fn model_material_options(ids: &mut Ids, primitive: bool) -> PropGroup {
    let mut g = ids
        .group("materialOptions", "Material Options")
        .with(ids.prop("castsShadows", "Casts Shadows", Value::Enum(0)).with_ui(popup(&["Off", "On", "Only"])))
        .with(ids.prop("acceptsShadows", "Accepts Shadows", Value::Enum(1)).with_ui(popup(&["Off", "On", "Only"])))
        .with(ids.prop("acceptsLights", "Accepts Lights", Value::Bool(true)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("appearsInReflections", "Appears in Reflections", Value::Bool(true)).with_ui(ParamUi::Checkbox));
    if primitive {
        g = g
            .with(ids.prop("baseColor", "Base Color", Value::Color([0.8, 0.8, 0.8, 1.0])).with_ui(ParamUi::Color))
            .with(ids.prop("metallic", "Metallic", Value::Scalar(0.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
            .with(ids.prop("roughness", "Roughness", Value::Scalar(50.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
            .with(ids.prop("emissive", "Emissive", Value::Color([0.0, 0.0, 0.0, 1.0])).with_ui(ParamUi::Color));
    }
    g
}

/// Geometry Options of an imported model layer: model units → pixels, and the animation clip
/// played along layer time.
pub fn model_geometry_options(ids: &mut Ids, unit_scale: f64, clips: &[String]) -> PropGroup {
    let mut opts: Vec<&str> = vec!["None"];
    opts.extend(clips.iter().map(String::as_str));
    ids.group("geometryOptions", "Geometry Options")
        .with(ids.prop("unitScale", "Model Scale", Value::Scalar(unit_scale)).with_ui(slider(0.0, 1.0e6, 0.0, 1000.0, 2)))
        .with(ids.prop("animation", "Animation", Value::Enum(if clips.is_empty() { 0 } else { 1 })).with_ui(popup(&opts)))
        .with(ids.prop("loopAnimation", "Loop Animation", Value::Bool(true)).with_ui(ParamUi::Checkbox))
        .with(ids.prop("animationSpeed", "Animation Speed", Value::Scalar(100.0)).with_ui(ParamUi::Percent))
}

/// Geometry Options of a primitive layer (pixels).
pub fn primitive_geometry_options(ids: &mut Ids, kind: crate::PrimitiveKind) -> PropGroup {
    use crate::PrimitiveKind as K;
    let px = |ids: &mut Ids, m: &str, n: &str, v: f64| ids.prop(m, n, Value::Scalar(v)).with_ui(slider(0.0, 100_000.0, 0.0, 2000.0, 1));
    let count = |ids: &mut Ids, m: &str, n: &str, v: f64| ids.prop(m, n, Value::Scalar(v)).with_ui(slider(3.0, 512.0, 3.0, 128.0, 0));
    let mut g = ids.group("geometryOptions", "Geometry Options");
    let props = match kind {
        K::Cube => vec![px(ids, "width", "Width", 300.0), px(ids, "height", "Height", 300.0), px(ids, "depth", "Depth", 300.0)],
        K::Plane => vec![px(ids, "width", "Width", 400.0), px(ids, "height", "Height", 400.0)],
        K::Sphere => vec![px(ids, "radius", "Radius", 150.0), count(ids, "segments", "Segments", 48.0), count(ids, "rings", "Rings", 24.0)],
        K::Torus => vec![
            px(ids, "radius", "Radius", 150.0),
            px(ids, "tubeRadius", "Tube Radius", 50.0),
            count(ids, "segments", "Segments", 48.0),
            count(ids, "rings", "Sides", 24.0),
        ],
        K::Cone | K::Cylinder => vec![px(ids, "radius", "Radius", 150.0), px(ids, "height", "Height", 300.0), count(ids, "segments", "Segments", 48.0)],
    };
    for p in props {
        g = g.with(p);
    }
    g
}

/// Geometry Options of a text or shape layer in an Advanced 3D comp (extrusion and bevels).
pub fn extrusion_geometry_options(ids: &mut Ids) -> PropGroup {
    ids.group("geometryOptions", "Geometry Options")
        .with(ids.prop("bevelStyle", "Bevel Style", Value::Enum(0)).with_ui(popup(&["None", "Angular", "Concave", "Convex"])))
        .with(ids.prop("bevelDepth", "Bevel Depth", Value::Scalar(2.0)).with_ui(slider(0.0, 1000.0, 0.0, 100.0, 1)))
        .with(ids.prop("holeBevelDepth", "Hole Bevel Depth", Value::Scalar(100.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)))
        .with(ids.prop("extrusionDepth", "Extrusion Depth", Value::Scalar(0.0)).with_ui(slider(0.0, 10_000.0, 0.0, 1000.0, 1)))
}

pub fn audio(ids: &mut Ids) -> PropGroup {
    ids.group("audio", "Audio").with(ids.prop("levels", "Audio Levels", Value::Vec2([0.0, 0.0])))
}

// ---------------------------------------------------------------- layers

/// Label colour for a new layer: its Project item's label when it has one (later changes to the
/// item's label leave existing layers alone), else the Labels preferences default for its type.
pub fn default_label(src: &LayerSource, project: &Project) -> Label {
    if let Some(it) = src.item().and_then(|i| project.item(i)) {
        return it.label;
    }
    match src {
        LayerSource::Comp { .. } => Label::Sandstone,
        LayerSource::Footage { .. } => Label::Aqua,
        LayerSource::Solid { .. } => Label::Red,
        LayerSource::Text => Label::Red,
        LayerSource::Shape => Label::Blue,
        LayerSource::Null => Label::Red,
        LayerSource::Camera | LayerSource::Light { .. } => Label::Pink,
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => Label::Aqua,
    }
}

/// A new layer spanning the whole comp, anchored at its source centre and centred in the comp.
pub fn layer(project: &mut Project, comp: &Comp, name: &str, source: LayerSource, size: (u32, u32), duration: Option<Tick>) -> Layer {
    let label = default_label(&source, project);
    let id = LayerId(project.alloc());
    let mut ids = Ids(&mut project.next_id);
    let (w, h) = (size.0 as f64, size.1 as f64);
    let (cw, ch) = (comp.width as f64, comp.height as f64);
    let anchor = match source {
        LayerSource::Text
        | LayerSource::Shape
        | LayerSource::Camera
        | LayerSource::Light { .. }
        | LayerSource::Model { .. }
        | LayerSource::Primitive { .. } => [0.0, 0.0],
        LayerSource::Null => [50.0, 50.0],
        _ => [w / 2.0, h / 2.0],
    };
    let mut root = ids.group("layer", name);
    match &source {
        LayerSource::Text => {
            root.children.push(text(&mut ids, TextDoc::default()).into());
        }
        LayerSource::Shape => {
            root.children.push(contents(&mut ids).into());
        }
        _ => {}
    }
    if source.is_model() {
        // Model layers have no masks or effects (as in After Effects).
    } else if source.is_av() {
        root.children.push(masks(&mut ids).into());
        root.children.push(effects(&mut ids).into());
    } else if matches!(source, LayerSource::Null) {
        root.children.push(effects(&mut ids).into());
    }
    let mut tr = transform(&mut ids, anchor, [cw / 2.0, ch / 2.0]);
    if let LayerSource::Camera = source {
        // Two-node camera default: point of interest at the comp centre, camera at -zoom.
        let zoom = effectcraft_geom::default_camera_zoom(cw);
        tr = ids
            .group("transform", "Transform")
            .with(ids.prop("poi", "Point of Interest", Value::Vec3([cw / 2.0, ch / 2.0, 0.0])).with_ui(ParamUi::Point3).spatial())
            .with(ids.prop("position", "Position", Value::Vec3([cw / 2.0, ch / 2.0, -zoom])).with_ui(ParamUi::Point3).spatial())
            .with(ids.prop("orientation", "Orientation", Value::Vec3([0.0; 3])).with_ui(ParamUi::Angle))
            .with(ids.prop("rotationX", "X Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
            .with(ids.prop("rotationY", "Y Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle))
            .with(ids.prop("rotation", "Z Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle));
        root.children.push(tr.into());
        root.children.push(camera_options_for(&mut ids, zoom, cw).into());
    } else if let LayerSource::Light { kind } = source {
        let mut g = ids.group("transform", "Transform");
        if matches!(kind, LightKind::Spot | LightKind::Parallel) {
            g.children.push(ids.prop("poi", "Point of Interest", Value::Vec3([cw / 2.0, ch / 2.0, 0.0])).with_ui(ParamUi::Point3).spatial().into());
        }
        if kind != LightKind::Ambient {
            g.children
                .push(ids.prop("position", "Position", Value::Vec3([cw / 2.0 - 260.0, ch / 2.0 - 260.0, -440.0])).with_ui(ParamUi::Point3).spatial().into());
            g.children.push(ids.prop("orientation", "Orientation", Value::Vec3([0.0; 3])).with_ui(ParamUi::Angle).into());
            g.children.push(ids.prop("rotationX", "X Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle).into());
            g.children.push(ids.prop("rotationY", "Y Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle).into());
            g.children.push(ids.prop("rotation", "Z Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle).into());
        }
        root.children.push(g.into());
        root.children.push(light_options(&mut ids, kind).into());
    } else {
        if let LayerSource::Null = source {
            // Nulls are 100×100.
        }
        root.children.push(std::mem::replace(&mut tr, PropGroup::new(0, "", "")).into());
        match source {
            LayerSource::Primitive { kind } => {
                root.children.push(primitive_geometry_options(&mut ids, kind).into());
                root.children.push(model_material_options(&mut ids, true).into());
            }
            LayerSource::Model { .. } => {
                root.children.push(model_geometry_options(&mut ids, 1.0, &[]).into());
                root.children.push(model_material_options(&mut ids, false).into());
            }
            _ if source.is_av() => root.children.push(material_options(&mut ids).into()),
            _ => {}
        }
    }
    let has_audio = matches!(&source, LayerSource::Footage { item } if matches!(project.item(*item).map(|i| &i.kind), Some(crate::ItemKind::Footage(f)) if f.has_audio))
        || matches!(source, LayerSource::Comp { .. });
    if has_audio {
        let mut ids = Ids(&mut project.next_id);
        root.children.push(audio(&mut ids).into());
    }
    let out = duration.map(|d| d.min(comp.duration)).unwrap_or(comp.duration);
    // Two-node cameras and spot/parallel lights aim at their Point of Interest.
    let auto_orient = if matches!(source, LayerSource::Camera | LayerSource::Light { kind: LightKind::Spot | LightKind::Parallel }) {
        crate::AutoOrient::TowardsPointOfInterest
    } else {
        crate::AutoOrient::Off
    };
    let switches = Switches {
        three_d: source.is_model(),
        collapse: matches!(source, LayerSource::Text | LayerSource::Shape),
        ..Switches::default()
    };
    Layer {
        id,
        name: comp.unique_layer_name(name),
        source,
        label,
        comment: String::new(),
        start_time: Tick::ZERO,
        in_point: Tick::ZERO,
        out_point: out,
        stretch: 100.0,
        switches,
        blend_mode: BlendMode::Normal,
        preserve_transparency: false,
        track_matte: None,
        parent: None,
        markers: vec![],
        markers_locked: false,
        environment: false,
        environment_background: false,
        auto_orient,
        props: root,
    }
}
