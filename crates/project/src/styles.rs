//! Layer Styles (Layer ▸ Layer Styles): the `layerStyles` property group and its styles.
//!
//! After Effects' layer styles are Photoshop's: Blending Options (Global Light, Advanced
//! Blending), Drop Shadow, Inner Shadow, Outer Glow, Inner Glow, Bevel and Emboss, Satin, Color
//! Overlay, Gradient Overlay and Stroke. Every style is a group under the layer's `layerStyles`
//! group (which sits after Transform, as in the AE timeline), with AE's property names, order,
//! defaults and ranges; every property animates. A style group's `enabled` flag is its eye
//! switch. Styles appear in the group in menu order whatever order they were added in.
//!
//! Global Light (angle/altitude) is one setting per composition: [`Comp::global_light`] holds it,
//! each layer's Blending Options shows it, and [`sync_global_light`] keeps every layer of a comp
//! in step after any edit.

use effectcraft_color::BlendMode;
use effectcraft_keyframe::{Gradient, Value};
use serde::{Deserialize, Serialize};

use crate::build::Ids;
use crate::props::{GroupKind, ParamUi, PropGroup, Property};
use crate::{Comp, Layer, Project};

/// Match id of the layer's Layer Styles group.
pub const GROUP: &str = "layerStyles";
/// Match id of the Blending Options group.
pub const BLENDING: &str = "blendingOptions";

/// The styles in menu / timeline order: (match id, display name).
pub const STYLES: &[(&str, &str)] = &[
    ("dropShadow", "Drop Shadow"),
    ("innerShadow", "Inner Shadow"),
    ("outerGlow", "Outer Glow"),
    ("innerGlow", "Inner Glow"),
    ("bevelEmboss", "Bevel and Emboss"),
    ("satin", "Satin"),
    ("colorOverlay", "Color Overlay"),
    ("gradientOverlay", "Gradient Overlay"),
    ("stroke", "Stroke"),
];

/// Look a style up by match id or display name (case-insensitive).
pub fn style_id(s: &str) -> Option<&'static str> {
    STYLES.iter().find(|(id, name)| id.eq_ignore_ascii_case(s) || name.eq_ignore_ascii_case(s)).map(|(id, _)| *id)
}

/// The Blend Mode popup of layer styles (Photoshop's list) and the compositor mode for each.
pub const STYLE_BLEND_MODES: &[(&str, BlendMode)] = &[
    ("Normal", BlendMode::Normal),
    ("Dissolve", BlendMode::Dissolve),
    ("Darken", BlendMode::Darken),
    ("Multiply", BlendMode::Multiply),
    ("Color Burn", BlendMode::ColorBurn),
    ("Linear Burn", BlendMode::LinearBurn),
    ("Darker Color", BlendMode::DarkerColor),
    ("Lighten", BlendMode::Lighten),
    ("Screen", BlendMode::Screen),
    ("Color Dodge", BlendMode::ColorDodge),
    ("Linear Dodge (Add)", BlendMode::LinearDodge),
    ("Lighter Color", BlendMode::LighterColor),
    ("Overlay", BlendMode::Overlay),
    ("Soft Light", BlendMode::SoftLight),
    ("Hard Light", BlendMode::HardLight),
    ("Vivid Light", BlendMode::VividLight),
    ("Linear Light", BlendMode::LinearLight),
    ("Pin Light", BlendMode::PinLight),
    ("Hard Mix", BlendMode::HardMix),
    ("Difference", BlendMode::Difference),
    ("Exclusion", BlendMode::Exclusion),
    ("Subtract", BlendMode::Subtract),
    ("Divide", BlendMode::Divide),
    ("Hue", BlendMode::Hue),
    ("Saturation", BlendMode::Saturation),
    ("Color", BlendMode::Color),
    ("Luminosity", BlendMode::Luminosity),
];

/// Compositor blend mode for a style Blend Mode popup index.
pub fn style_blend_mode(i: u32) -> BlendMode {
    STYLE_BLEND_MODES.get(i as usize).map(|m| m.1).unwrap_or(BlendMode::Normal)
}

fn mode_index(m: BlendMode) -> u32 {
    STYLE_BLEND_MODES.iter().position(|x| x.1 == m).unwrap_or(0) as u32
}

pub const KNOCKOUT: &[&str] = &["None", "Shallow", "Deep"];
pub const COLOR_TYPE: &[&str] = &["Color", "Gradient"];
pub const GLOW_TECHNIQUE: &[&str] = &["Softer", "Precise"];
pub const GLOW_SOURCE: &[&str] = &["Center", "Edge"];
pub const BEVEL_STYLE: &[&str] = &["Outer Bevel", "Inner Bevel", "Emboss", "Pillow Emboss", "Stroke Emboss"];
pub const BEVEL_TECHNIQUE: &[&str] = &["Smooth", "Chisel Hard", "Chisel Soft"];
pub const BEVEL_DIRECTION: &[&str] = &["Up", "Down"];
pub const GRADIENT_STYLE: &[&str] = &["Linear", "Radial", "Angle", "Reflected", "Diamond"];
pub const STROKE_POSITION: &[&str] = &["Outside", "Inside", "Center"];

/// Default Global Light (Photoshop / After Effects defaults).
pub const DEFAULT_ANGLE: f64 = 120.0;
pub const DEFAULT_ALTITUDE: f64 = 30.0;

/// A composition's Global Light, mirrored into every layer's Blending Options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlobalLight {
    pub angle: Property,
    pub altitude: Property,
}

impl Default for GlobalLight {
    fn default() -> Self {
        GlobalLight {
            angle: Property::new(0, "globalLightAngle", "Global Light Angle", Value::Scalar(DEFAULT_ANGLE)).with_ui(ParamUi::Angle),
            altitude: Property::new(0, "globalLightAltitude", "Global Light Altitude", Value::Scalar(DEFAULT_ALTITUDE)).with_ui(ParamUi::Angle),
        }
    }
}

fn slider(min: f64, max: f64, decimals: u8) -> ParamUi {
    ParamUi::Slider { min, max, slider_min: min, slider_max: max, decimals }
}
fn slider2(min: f64, max: f64, smin: f64, smax: f64, decimals: u8) -> ParamUi {
    ParamUi::Slider { min, max, slider_min: smin, slider_max: smax, decimals }
}
fn popup(opts: &[&str]) -> ParamUi {
    ParamUi::Popup { options: opts.iter().map(|s| s.to_string()).collect() }
}
fn style_modes() -> ParamUi {
    ParamUi::Popup { options: STYLE_BLEND_MODES.iter().map(|m| m.0.to_string()).collect() }
}
fn rgb(r: u8, g: u8, b: u8) -> Value {
    Value::Color([r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0])
}

fn mode(ids: &mut Ids, m: &str, name: &str, d: BlendMode) -> Property {
    ids.prop(m, name, Value::Enum(mode_index(d))).with_ui(style_modes())
}
fn color(ids: &mut Ids, m: &str, name: &str, v: Value) -> Property {
    ids.prop(m, name, v).with_ui(ParamUi::Color)
}
fn pct(ids: &mut Ids, m: &str, name: &str, v: f64) -> Property {
    ids.prop(m, name, Value::Scalar(v)).with_ui(slider(0.0, 100.0, 0))
}
fn px(ids: &mut Ids, m: &str, name: &str, v: f64, max: f64) -> Property {
    ids.prop(m, name, Value::Scalar(v)).with_ui(slider(0.0, max, 1))
}
fn check(ids: &mut Ids, m: &str, name: &str, v: bool) -> Property {
    ids.prop(m, name, Value::Bool(v)).with_ui(ParamUi::Checkbox)
}
fn pop(ids: &mut Ids, m: &str, name: &str, v: u32, opts: &[&str]) -> Property {
    ids.prop(m, name, Value::Enum(v)).with_ui(popup(opts))
}
fn angle(ids: &mut Ids, m: &str, name: &str, v: f64) -> Property {
    ids.prop(m, name, Value::Scalar(v)).with_ui(ParamUi::Angle)
}
fn gradient(ids: &mut Ids, g: Gradient) -> Property {
    ids.prop("colors", "Colors", Value::Gradient(g)).with_ui(ParamUi::Gradient)
}

/// The default glow gradient: the glow colour fading to transparent.
fn glow_gradient() -> Gradient {
    let c = [1.0, 1.0, 190.0 / 255.0, 1.0];
    Gradient { colors: vec![(0.0, c), (1.0, c)], opacities: vec![(0.0, 1.0), (1.0, 0.0)] }
}

/// Global Light properties as they appear on a layer (copied from the comp's setting).
fn global_props(ids: &mut Ids, gl: &GlobalLight) -> (Property, Property) {
    let mut a = gl.angle.clone();
    a.uid = ids.alloc();
    let mut b = gl.altitude.clone();
    b.uid = ids.alloc();
    (a, b)
}

/// The Blending Options group.
pub fn blending_options(ids: &mut Ids, gl: &GlobalLight) -> PropGroup {
    let (a, b) = global_props(ids, gl);
    let adv = ids
        .group("advancedBlending", "Advanced Blending")
        .with(pct(ids, "fillOpacity", "Fill Opacity", 100.0))
        .with(check(ids, "red", "Red", true))
        .with(check(ids, "green", "Green", true))
        .with(check(ids, "blue", "Blue", true))
        .with(pop(ids, "knockout", "Knockout", 0, KNOCKOUT))
        .with(check(ids, "blendInteriorAsGroup", "Blend Interior Styles as Group", false))
        .with(check(ids, "useBlendRanges", "Use Blend Ranges from Source", true));
    ids.group(BLENDING, "Blending Options").with(a).with(b).with(adv)
}

/// An empty Layer Styles group (Blending Options only).
pub fn layer_styles(ids: &mut Ids, gl: &GlobalLight) -> PropGroup {
    let bo = blending_options(ids, gl);
    ids.group(GROUP, "Layer Styles").with(bo)
}

/// A style group with AE's defaults (`id` from [`STYLES`]).
pub fn style(ids: &mut Ids, id: &str) -> Option<PropGroup> {
    let (_, name) = STYLES.iter().find(|s| s.0 == id)?;
    let mut g = ids.group(id, name);
    g.kind = GroupKind::Plain;
    let props: Vec<Property> = match id {
        "dropShadow" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Multiply),
            color(ids, "color", "Color", rgb(0, 0, 0)),
            pct(ids, "opacity", "Opacity", 75.0),
            check(ids, "useGlobalLight", "Use Global Light", true),
            angle(ids, "angle", "Angle", DEFAULT_ANGLE),
            ids.prop("distance", "Distance", Value::Scalar(5.0)).with_ui(slider2(0.0, 30000.0, 0.0, 100.0, 1)),
            pct(ids, "spread", "Spread", 0.0),
            px(ids, "size", "Size", 5.0, 250.0),
            pct(ids, "noise", "Noise", 0.0),
            check(ids, "knocksOut", "Layer Knocks Out Drop Shadow", true),
        ],
        "innerShadow" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Multiply),
            color(ids, "color", "Color", rgb(0, 0, 0)),
            pct(ids, "opacity", "Opacity", 75.0),
            check(ids, "useGlobalLight", "Use Global Light", true),
            angle(ids, "angle", "Angle", DEFAULT_ANGLE),
            ids.prop("distance", "Distance", Value::Scalar(5.0)).with_ui(slider2(0.0, 30000.0, 0.0, 100.0, 1)),
            pct(ids, "choke", "Choke", 0.0),
            px(ids, "size", "Size", 5.0, 250.0),
            pct(ids, "noise", "Noise", 0.0),
        ],
        "outerGlow" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Screen),
            pct(ids, "opacity", "Opacity", 75.0),
            pct(ids, "noise", "Noise", 0.0),
            pop(ids, "colorType", "Color Type", 0, COLOR_TYPE),
            color(ids, "color", "Color", rgb(255, 255, 190)),
            gradient(ids, glow_gradient()),
            pct(ids, "gradientSmoothness", "Gradient Smoothness", 100.0),
            pop(ids, "technique", "Technique", 0, GLOW_TECHNIQUE),
            pct(ids, "spread", "Spread", 0.0),
            px(ids, "size", "Size", 5.0, 250.0),
            ids.prop("range", "Range", Value::Scalar(50.0)).with_ui(slider(1.0, 100.0, 0)),
            pct(ids, "jitter", "Jitter", 0.0),
        ],
        "innerGlow" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Screen),
            pct(ids, "opacity", "Opacity", 75.0),
            pct(ids, "noise", "Noise", 0.0),
            pop(ids, "colorType", "Color Type", 0, COLOR_TYPE),
            color(ids, "color", "Color", rgb(255, 255, 190)),
            gradient(ids, glow_gradient()),
            pct(ids, "gradientSmoothness", "Gradient Smoothness", 100.0),
            pop(ids, "technique", "Technique", 0, GLOW_TECHNIQUE),
            pop(ids, "source", "Source", 1, GLOW_SOURCE),
            pct(ids, "choke", "Choke", 0.0),
            px(ids, "size", "Size", 5.0, 250.0),
            ids.prop("range", "Range", Value::Scalar(50.0)).with_ui(slider(1.0, 100.0, 0)),
            pct(ids, "jitter", "Jitter", 0.0),
        ],
        "bevelEmboss" => vec![
            pop(ids, "style", "Style", 1, BEVEL_STYLE),
            pop(ids, "technique", "Technique", 0, BEVEL_TECHNIQUE),
            ids.prop("depth", "Depth", Value::Scalar(100.0)).with_ui(slider2(1.0, 1000.0, 1.0, 1000.0, 0)),
            pop(ids, "direction", "Direction", 0, BEVEL_DIRECTION),
            px(ids, "size", "Size", 5.0, 250.0),
            px(ids, "soften", "Soften", 0.0, 16.0),
            check(ids, "useGlobalLight", "Use Global Light", true),
            angle(ids, "angle", "Angle", DEFAULT_ANGLE),
            ids.prop("altitude", "Altitude", Value::Scalar(DEFAULT_ALTITUDE)).with_ui(ParamUi::Angle),
            mode(ids, "highlightMode", "Highlight Mode", BlendMode::Screen),
            color(ids, "highlightColor", "Highlight Color", rgb(255, 255, 255)),
            pct(ids, "highlightOpacity", "Highlight Opacity", 75.0),
            mode(ids, "shadowMode", "Shadow Mode", BlendMode::Multiply),
            color(ids, "shadowColor", "Shadow Color", rgb(0, 0, 0)),
            pct(ids, "shadowOpacity", "Shadow Opacity", 75.0),
        ],
        "satin" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Multiply),
            color(ids, "color", "Color", rgb(0, 0, 0)),
            pct(ids, "opacity", "Opacity", 50.0),
            angle(ids, "angle", "Angle", 19.0),
            px(ids, "distance", "Distance", 11.0, 250.0),
            px(ids, "size", "Size", 14.0, 250.0),
            check(ids, "invert", "Invert", true),
        ],
        "colorOverlay" => {
            vec![mode(ids, "blendMode", "Blend Mode", BlendMode::Normal), color(ids, "color", "Color", rgb(255, 0, 0)), pct(ids, "opacity", "Opacity", 100.0)]
        }
        "gradientOverlay" => vec![
            mode(ids, "blendMode", "Blend Mode", BlendMode::Normal),
            pct(ids, "opacity", "Opacity", 100.0),
            gradient(ids, Gradient { colors: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] }),
            pct(ids, "gradientSmoothness", "Gradient Smoothness", 100.0),
            angle(ids, "angle", "Angle", 90.0),
            pop(ids, "style", "Style", 0, GRADIENT_STYLE),
            check(ids, "reverse", "Reverse", false),
            check(ids, "alignWithLayer", "Align with Layer", true),
            ids.prop("scale", "Scale", Value::Scalar(100.0)).with_ui(slider(10.0, 150.0, 0)),
            ids.prop("offset", "Offset", Value::Vec2([0.0, 0.0])).with_ui(ParamUi::Point),
        ],
        "stroke" => vec![
            color(ids, "color", "Color", rgb(255, 0, 0)),
            mode(ids, "blendMode", "Blend Mode", BlendMode::Normal),
            ids.prop("size", "Size", Value::Scalar(3.0)).with_ui(slider(1.0, 250.0, 1)),
            pct(ids, "opacity", "Opacity", 100.0),
            pop(ids, "position", "Position", 0, STROKE_POSITION),
        ],
        _ => return None,
    };
    for p in props {
        g.children.push(p.into());
    }
    Some(g)
}

impl Layer {
    /// The Layer Styles group (present once a style was added).
    pub fn layer_styles(&self) -> Option<&PropGroup> {
        self.props.sub(GROUP)
    }
    /// Layer types that take layer styles (AV layers: footage, solids, precomps, text, shapes).
    pub fn can_have_styles(&self) -> bool {
        self.source.is_av() && !self.switches.adjustment
    }
}

/// Add style `id` to `layer` (creating the Layer Styles group after Transform when missing).
/// Returns the style group's uid; an existing style of that kind is kept and returned.
pub fn add_style(layer: &mut Layer, ids: &mut Ids, id: &str, gl: &GlobalLight, enabled: bool) -> Option<u64> {
    let order = STYLES.iter().position(|s| s.0 == id)?;
    if layer.props.sub(GROUP).is_none() {
        let g = layer_styles(ids, gl);
        let at = layer.props.children.iter().position(|c| c.match_id() == "transform").map(|i| i + 1).unwrap_or(layer.props.children.len());
        layer.props.children.insert(at, g.into());
    }
    let ls = layer.props.sub_mut(GROUP)?;
    if let Some(g) = ls.sub(id) {
        return Some(g.uid);
    }
    let mut g = style(ids, id)?;
    g.enabled = enabled;
    let uid = g.uid;
    // Keep menu order: after Blending Options and every style that comes earlier.
    let at = ls.children.iter().position(|c| STYLES.iter().position(|s| s.0 == c.match_id()).is_some_and(|o| o > order)).unwrap_or(ls.children.len());
    ls.children.insert(at, g.into());
    Some(uid)
}

fn same_anim(a: &Property, b: &Property) -> bool {
    a.value == b.value && a.keys == b.keys && a.expr == b.expr
}

fn copy_anim(dst: &mut Property, src: &Property) {
    dst.value = src.value.clone();
    dst.keys = src.keys.clone();
    dst.expr = src.expr.clone();
}

/// After an edit: if a layer's Global Light Angle/Altitude differs from its comp's setting, that
/// layer was edited — adopt its value for the comp and copy it to every other layer (`before` is
/// the project before the edit; comps that did not change are skipped).
pub fn sync_global_light(before: &Project, after: &mut Project) {
    let ids: Vec<_> = after
        .items
        .iter()
        .filter_map(|(id, it)| match (&it.kind, before.items.get(id).map(|b| &b.kind)) {
            (crate::ItemKind::Comp(a), Some(crate::ItemKind::Comp(b))) if std::sync::Arc::ptr_eq(a, b) => None,
            (crate::ItemKind::Comp(_), _) => Some(*id),
            _ => None,
        })
        .collect();
    for id in ids {
        let Some(comp) = after.comp(id) else { continue };
        if !needs_sync(comp) {
            continue;
        }
        if let Some(c) = after.comp_mut(id) {
            sync_comp(c);
        }
    }
}

fn needs_sync(comp: &Comp) -> bool {
    comp.layers.iter().filter_map(|l| l.layer_styles()?.sub(BLENDING)).any(|bo| {
        bo.get("globalLightAngle").is_some_and(|p| !same_anim(p, &comp.global_light.angle))
            || bo.get("globalLightAltitude").is_some_and(|p| !same_anim(p, &comp.global_light.altitude))
    })
}

/// Make every layer of `comp` share one Global Light (the first layer that differs wins).
pub fn sync_comp(comp: &mut Comp) {
    for (m, which) in [("globalLightAngle", 0), ("globalLightAltitude", 1)] {
        let cur = if which == 0 { &comp.global_light.angle } else { &comp.global_light.altitude };
        let changed = comp.layers.iter().filter_map(|l| l.layer_styles()?.sub(BLENDING)?.get(m)).find(|p| !same_anim(p, cur)).cloned();
        let Some(src) = changed else { continue };
        let target = if which == 0 { &mut comp.global_light.angle } else { &mut comp.global_light.altitude };
        copy_anim(target, &src);
        for l in &mut comp.layers {
            if let Some(p) = l.props.sub_mut(GROUP).and_then(|g| g.sub_mut(BLENDING)).and_then(|g| g.get_mut(m)) {
                copy_anim(p, &src);
            }
        }
    }
}

/// Set the comp's Global Light statically (`None` keeps a value) and mirror it to all layers.
pub fn set_global_light(comp: &mut Comp, angle: Option<f64>, altitude: Option<f64>, t: effectcraft_time::Tick) {
    if let Some(a) = angle {
        comp.global_light.angle.set_value_at(t, Value::Scalar(a));
    }
    if let Some(a) = altitude {
        comp.global_light.altitude.set_value_at(t, Value::Scalar(a.clamp(0.0, 90.0)));
    }
    let (a, b) = (comp.global_light.angle.clone(), comp.global_light.altitude.clone());
    for l in &mut comp.layers {
        if let Some(bo) = l.props.sub_mut(GROUP).and_then(|g| g.sub_mut(BLENDING)) {
            if let Some(p) = bo.get_mut("globalLightAngle") {
                copy_anim(p, &a);
            }
            if let Some(p) = bo.get_mut("globalLightAltitude") {
                copy_anim(p, &b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerSource, build};
    use effectcraft_time::{FrameRate, Tick};

    fn setup() -> (Project, Comp, Layer) {
        let mut p = Project::default();
        let comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let l = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (0, 0), None);
        (p, comp, l)
    }

    #[test]
    fn styles_keep_menu_order_and_follow_transform() {
        let (mut p, comp, mut l) = setup();
        let gl = comp.global_light.clone();
        let mut ids = Ids(&mut p.next_id);
        add_style(&mut l, &mut ids, "stroke", &gl, true).unwrap();
        add_style(&mut l, &mut ids, "dropShadow", &gl, true).unwrap();
        let again = add_style(&mut l, &mut ids, "stroke", &gl, true).unwrap();
        add_style(&mut l, &mut ids, "bevelEmboss", &gl, true).unwrap();
        let ls = l.layer_styles().unwrap();
        let order: Vec<&str> = ls.children.iter().map(|c| c.match_id()).collect();
        assert_eq!(order, ["blendingOptions", "dropShadow", "bevelEmboss", "stroke"]);
        assert_eq!(ls.sub("stroke").unwrap().uid, again);
        let top: Vec<&str> = l.props.children.iter().map(|c| c.match_id()).collect();
        let t = top.iter().position(|m| *m == "transform").unwrap();
        assert_eq!(top[t + 1], GROUP);
        // Defaults from AE.
        let ds = ls.sub("dropShadow").unwrap();
        assert_eq!(ds.get("opacity").unwrap().value, Value::Scalar(75.0));
        assert_eq!(style_blend_mode(ds.get("blendMode").unwrap().value.as_enum()), BlendMode::Multiply);
        assert_eq!(ls.sub(BLENDING).unwrap().get("globalLightAngle").unwrap().value, Value::Scalar(120.0));
    }

    #[test]
    fn every_style_builds() {
        let mut next = 1;
        let mut ids = Ids(&mut next);
        for (id, name) in STYLES {
            let g = style(&mut ids, id).unwrap();
            assert_eq!(g.name, *name);
            assert!(!g.children.is_empty());
        }
        assert_eq!(style_id("Bevel and Emboss"), Some("bevelEmboss"));
    }

    #[test]
    fn global_light_syncs_across_layers() {
        let (mut p, mut comp, mut a) = setup();
        let mut b = build::layer(&mut p, &comp, "B", LayerSource::Shape, (0, 0), None);
        let gl = comp.global_light.clone();
        let mut ids = Ids(&mut p.next_id);
        add_style(&mut a, &mut ids, "dropShadow", &gl, true);
        add_style(&mut b, &mut ids, "bevelEmboss", &gl, true);
        comp.layers = vec![a, b].into();
        comp.layers[1].props.sub_mut(GROUP).unwrap().sub_mut(BLENDING).unwrap().get_mut("globalLightAngle").unwrap().value = Value::Scalar(45.0);
        assert!(needs_sync(&comp));
        sync_comp(&mut comp);
        assert!(!needs_sync(&comp));
        assert_eq!(comp.global_light.angle.value, Value::Scalar(45.0));
        let la = comp.layers[0].layer_styles().unwrap().sub(BLENDING).unwrap().get("globalLightAngle").unwrap();
        assert_eq!(la.value, Value::Scalar(45.0));
        set_global_light(&mut comp, None, Some(60.0), Tick::ZERO);
        let lb = comp.layers[1].layer_styles().unwrap().sub(BLENDING).unwrap().get("globalLightAltitude").unwrap();
        assert_eq!(lb.value, Value::Scalar(60.0));
    }
}
