//! Layer styles (951-979): the applied styles arrive with After Effects' defaults and the stored
//! values, keys and Blending Options, without losses.

mod common;

use common::{comp, try_import};
use effectcraft_keyframe::Value;
use effectcraft_project::styles::{BLENDING, GROUP, STYLE_BLEND_MODES};
use effectcraft_project::{Comp, Layer, Project, PropGroup};
use effectcraft_time::Tick;

fn lossless(name: &str) -> (Project, Comp) {
    let mut p = Project::default();
    let r = try_import(&mut p, name, false).unwrap_or_else(|e| panic!("{name}: lossless import refused: {e}"));
    let (_, c) = comp(&p, &r, "C");
    let c = c.clone();
    (p, c)
}

fn b(c: &Comp) -> &Layer {
    c.layers.iter().find(|l| l.name == "B").expect("B")
}

fn style<'a>(l: &'a Layer, id: &str) -> &'a PropGroup {
    l.props.sub(GROUP).and_then(|g| g.sub(id)).unwrap_or_else(|| panic!("no style {id}"))
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.get(id).unwrap_or_else(|| panic!("no {id} in {}", g.match_id)).value.clone()
}

fn mode(name: &str) -> Value {
    Value::Enum(STYLE_BLEND_MODES.iter().position(|m| m.0 == name).expect("mode") as u32)
}

fn s(x: f64) -> Value {
    Value::Scalar(x)
}

fn rgb(r: f64, g: f64, b: f64) -> Value {
    Value::Color([r, g, b, 1.0])
}

#[test]
fn all_nine_at_after_effects_defaults() {
    let (_, c) = lossless("951-styles-all-nine-defaults-on-B.aep");
    let l = b(&c);
    let ls = l.props.sub(GROUP).expect("layer styles");
    let ids: Vec<&str> = ls.children.iter().map(|n| n.match_id()).collect();
    assert_eq!(ids, [BLENDING, "dropShadow", "innerShadow", "outerGlow", "innerGlow", "bevelEmboss", "satin", "colorOverlay", "gradientOverlay", "stroke"]);
    assert!(ls.groups().all(|g| g.enabled));
    let ds = style(l, "dropShadow");
    assert_eq!((v(ds, "blendMode"), v(ds, "opacity"), v(ds, "angle")), (mode("Multiply"), s(75.0), s(120.0)));
    // After Effects' Use Global Light is off by default (EffectCraft's own default is on).
    assert_eq!(v(ds, "useGlobalLight"), Value::Bool(false));
    assert_eq!(v(style(l, "outerGlow"), "blendMode"), mode("Screen"));
    assert_eq!(v(style(l, "innerGlow"), "source"), Value::Enum(1), "Edge");
    assert_eq!(v(style(l, "bevelEmboss"), "style"), Value::Enum(1), "Inner Bevel");
    assert_eq!(v(style(l, "bevelEmboss"), "useGlobalLight"), Value::Bool(false));
    assert_eq!(v(style(l, "satin"), "invert"), Value::Bool(true));
    assert_eq!(v(style(l, "colorOverlay"), "color"), rgb(1.0, 0.0, 0.0));
    assert_eq!(v(style(l, "stroke"), "size"), s(3.0));
    // The footage layer S has none.
    let sl = c.layers.iter().find(|l| l.name == "S").expect("S");
    assert!(sl.props.sub(GROUP).is_none());
}

#[test]
fn stored_values_in_ui_units() {
    let (_, c) = lossless("966-style-drop-shadow-opacity50-distance20.aep");
    let l = b(&c);
    assert_eq!(l.props.sub(GROUP).map(|g| g.children.len()), Some(2), "Blending Options + Drop Shadow");
    let ds = style(l, "dropShadow");
    assert_eq!((v(ds, "opacity"), v(ds, "distance"), v(ds, "size")), (s(50.0), s(20.0), s(5.0)));
    let (_, c) = lossless("970-style-stroke-size10-color-green.aep");
    let st = style(b(&c), "stroke");
    assert_eq!((v(st, "size"), v(st, "color")), (s(10.0), rgb(0.0, 1.0, 0.0)));
    let (_, c) = lossless("974-style-gradient-overlay-angle45-scale150.aep");
    let go = style(b(&c), "gradientOverlay");
    assert_eq!((v(go, "angle"), v(go, "scale")), (s(45.0), s(150.0)));
    let (_, c) = lossless("975-style-fill-opacity0-with-drop-shadow.aep");
    let adv = b(&c).props.sub(GROUP).and_then(|g| g.sub(BLENDING)).and_then(|g| g.sub("advancedBlending")).expect("advanced");
    assert_eq!(v(adv, "fillOpacity"), s(0.0));
    let (_, c) = lossless("979-style-inner-glow-source-2.aep");
    assert_eq!(v(style(b(&c), "innerGlow"), "source"), Value::Enum(0), "Center");
}

#[test]
fn keyed_opacity() {
    let (_, c) = lossless("976-style-drop-shadow-opacity-keyed-100-20-linear.aep");
    let p = style(b(&c), "dropShadow").get("opacity").expect("opacity");
    assert_eq!(p.keys.len(), 2);
    assert_eq!(p.value_at(Tick::from_seconds_f64(1.0)), s(60.0), "After Effects reads 60 at 1 s");
}

#[test]
fn every_parameter_of_977() {
    let (_, c) = lossless("977-styles-all-params-changed.aep");
    let l = b(&c);
    // Global Light is the composition's and every layer's Blending Options mirror it.
    assert_eq!((c.global_light.angle.value.clone(), c.global_light.altitude.value.clone()), (s(45.0), s(40.0)));
    let bo = l.props.sub(GROUP).and_then(|g| g.sub(BLENDING)).expect("blending");
    assert_eq!(v(bo, "globalLightAngle"), s(45.0));
    let adv = bo.sub("advancedBlending").expect("advanced");
    assert_eq!(
        (v(adv, "fillOpacity"), v(adv, "red"), v(adv, "green"), v(adv, "blendInteriorAsGroup"), v(adv, "useBlendRanges")),
        (s(80.0), Value::Bool(false), Value::Bool(true), Value::Bool(true), Value::Bool(false))
    );
    let ds = style(l, "dropShadow");
    assert_eq!(
        [
            v(ds, "blendMode"),
            v(ds, "color"),
            v(ds, "opacity"),
            v(ds, "useGlobalLight"),
            v(ds, "angle"),
            v(ds, "distance"),
            v(ds, "spread"),
            v(ds, "size"),
            v(ds, "noise"),
            v(ds, "knocksOut")
        ],
        [mode("Normal"), rgb(0.0, 0.0, 1.0), s(60.0), Value::Bool(true), s(30.0), s(12.0), s(10.0), s(8.0), s(5.0), Value::Bool(false)]
    );
    let og = style(l, "outerGlow");
    assert_eq!(
        [v(og, "colorType"), v(og, "technique"), v(og, "range"), v(og, "jitter"), v(og, "gradientSmoothness")],
        [Value::Enum(1), Value::Enum(1), s(40.0), s(10.0), s(80.0)]
    );
    assert_eq!(v(style(l, "innerGlow"), "source"), Value::Enum(0));
    let be = style(l, "bevelEmboss");
    assert_eq!(
        [
            v(be, "style"),
            v(be, "technique"),
            v(be, "depth"),
            v(be, "direction"),
            v(be, "soften"),
            v(be, "altitude"),
            v(be, "highlightMode"),
            v(be, "shadowColor")
        ],
        [Value::Enum(0), Value::Enum(1), s(150.0), Value::Enum(1), s(3.0), s(50.0), mode("Normal"), rgb(0.0, 0.0, 1.0)]
    );
    let sa = style(l, "satin");
    assert_eq!([v(sa, "angle"), v(sa, "invert"), v(sa, "color")], [s(45.0), Value::Bool(false), rgb(1.0, 0.0, 1.0)]);
    let co = style(l, "colorOverlay");
    assert_eq!([v(co, "blendMode"), v(co, "opacity")], [mode("Multiply"), s(50.0)]);
    let go = style(l, "gradientOverlay");
    assert_eq!(
        [v(go, "style"), v(go, "reverse"), v(go, "alignWithLayer"), v(go, "offset"), v(go, "angle")],
        [Value::Enum(1), Value::Bool(true), Value::Bool(false), Value::Vec2([10.0, 20.0]), s(30.0)]
    );
    let st = style(l, "stroke");
    assert_eq!([v(st, "position"), v(st, "size"), v(st, "blendMode")], [Value::Enum(1), s(6.0), mode("Multiply")]);
}

#[test]
fn a_switched_off_style_arrives_switched_off() {
    // 988: the Drop Shadow's switch group at `tdsb` 0 (After Effects does not draw it).
    let (_, c) = lossless("988-style-drop-shadow-switched-off.aep");
    let ds = style(b(&c), "dropShadow");
    assert!(!ds.enabled);
    assert_eq!(v(ds, "opacity"), s(75.0));
}
