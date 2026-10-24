//! The effects of batch 5 (409-450, 511-516) and the expression controls and Fill of earlier
//! batches import without losses with the values they were made with.

mod common;

use common::{comp, try_import};
use effectcraft_keyframe::Value;
use effectcraft_project::{Project, PropGroup};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

/// The only effect of layer L, imported without losses.
fn only_effect(name: &str) -> PropGroup {
    let mut p = Project::default();
    let r = try_import(&mut p, name, false).unwrap_or_else(|e| panic!("{name}: lossless import refused: {e}"));
    let (_, c) = comp(&p, &r, "C");
    let l = c.layers.iter().find(|l| l.name == "L").expect("L");
    let fx = l.props.sub("effects").expect("effects");
    let groups: Vec<_> = fx.children.iter().filter_map(|n| n.as_group()).cloned().collect();
    assert_eq!(groups.len(), 1, "{name}");
    groups.into_iter().next().expect("effect")
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id} in {}", g.match_id)).value.clone()
}

fn s(x: f64) -> Value {
    Value::Scalar(x)
}

fn close(a: &Value, b: f64) {
    let Value::Scalar(a) = a else { panic!("{a:?}") };
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn invert() {
    let g = only_effect("410-fx-invert-channel-2.aep");
    assert_eq!(g.match_id, "ec.channel.invert");
    // The second file item, Red, is EffectCraft's item 1 (the separators are skipped by name).
    assert_eq!((v(&g, "channel"), v(&g, "blend")), (Value::Enum(1), s(0.0)));
    let g = only_effect("411-fx-invert-blend-50.aep");
    assert_eq!((v(&g, "channel"), v(&g, "blend")), (Value::Enum(0), s(50.0)));
}

#[test]
fn fast_box_blur_and_gaussian_blur() {
    let g = only_effect("413-fx-fastbox-radius-20.aep");
    assert_eq!(
        (v(&g, "radius"), v(&g, "iterations"), v(&g, "dimensions"), v(&g, "repeatEdge")),
        (s(20.0), s(3.0), Value::Enum(0), Value::Bool(true)),
        "Repeat Edge Pixels is on by default"
    );
    assert_eq!(v(&only_effect("414-fx-fastbox-iterations-1.aep"), "iterations"), s(1.0));
    assert_eq!(v(&only_effect("416-fx-fastbox-repeatedge-off.aep"), "repeatEdge"), Value::Bool(false));

    let g = only_effect("418-fx-gaussian-dimensions-3.aep");
    assert_eq!((v(&g, "blurriness"), v(&g, "dimensions"), v(&g, "repeatEdge")), (s(25.0), Value::Enum(2), Value::Bool(true)));
    assert_eq!(v(&only_effect("419-fx-gaussian-repeatedge-off.aep"), "repeatEdge"), Value::Bool(false));
    assert_eq!(v(&only_effect("216-fx-gaussianblur-40.aep"), "blurriness"), s(40.0));
    assert_eq!(v(&only_effect("214-fx-gaussianblur-default.aep"), "blurriness"), s(25.0));
}

#[test]
fn drop_shadow() {
    let g = only_effect("422-fx-dropshadow-opacity-204.aep");
    assert_eq!(g.match_id, "ec.perspective.dropshadow");
    close(&v(&g, "opacity"), 80.0);
    assert_eq!((v(&g, "direction"), v(&g, "distance"), v(&g, "softness"), v(&g, "shadowOnly")), (s(135.0), s(5.0), s(0.0), Value::Bool(false)));
    assert_eq!(v(&g, "color"), Value::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(v(&only_effect("426-fx-dropshadow-shadowonly-on.aep"), "shadowOnly"), Value::Bool(true));
    close(&v(&only_effect("080-fx-dropshadow-default.aep"), "opacity"), 50.0);
}

#[test]
fn glow_defaults_follow_after_effects() {
    let g = only_effect("427-fx-glow-default.aep");
    assert_eq!(g.match_id, "ec.stylize.glow");
    assert_eq!(v(&g, "based"), Value::Enum(1), "Color Channels");
    close(&v(&g, "threshold"), 60.0);
    assert_eq!((v(&g, "radius"), v(&g, "intensity")), (s(10.0), s(1.0)));
    assert_eq!(v(&g, "operation"), Value::Enum(1), "Composite Original: Behind");
    assert_eq!(v(&g, "glowOperation"), Value::Enum(1), "Add");
    assert_eq!((v(&g, "colors"), v(&g, "colorLooping")), (Value::Enum(0), Value::Enum(2)));
    assert_eq!((v(&g, "colorLoops"), v(&g, "colorPhase")), (s(1.0), s(0.0)));
    close(&v(&g, "abMidpoint"), 50.0);
    assert_eq!((v(&g, "colorA"), v(&g, "colorB"), v(&g, "glowDimensions")), (Value::Color([1.0; 4]), Value::Color([0.0, 0.0, 0.0, 1.0]), Value::Enum(0)));
    close(&v(&only_effect("429-fx-glow-threshold-76.5.aep"), "threshold"), 30.0);
    close(&v(&only_effect("438-fx-glow-midpoint-0.25.aep"), "abMidpoint"), 25.0);
}

#[test]
fn hue_saturation_master_values_and_colorize() {
    let g = only_effect("444-fx-huesat-hue-90.aep");
    assert_eq!(g.match_id, "ec.color.huesaturation");
    assert_eq!((v(&g, "hue"), v(&g, "saturation"), v(&g, "lightness")), (s(90.0), s(0.0), s(0.0)));
    assert_eq!(v(&only_effect("446-fx-huesat-light-m30.aep"), "lightness"), s(-30.0));
    let g = only_effect("448-fx-huesat-colorizehue-120.aep");
    assert_eq!((v(&g, "colorize"), v(&g, "colorizeHue"), v(&g, "colorizeSaturation"), v(&g, "hue")), (Value::Bool(false), s(120.0), s(25.0), s(0.0)));
}

#[test]
fn levels_values_from_the_data() {
    let f32r = |x: f64| f64::from(x as f32);
    let g = only_effect("512-fx-levels-inblack-0.1.aep");
    assert_eq!(g.match_id, "ec.color.levels");
    assert_eq!((v(&g, "inBlack"), v(&g, "inWhite"), v(&g, "gamma"), v(&g, "outBlack"), v(&g, "outWhite")), (s(f32r(0.1)), s(1.0), s(1.0), s(0.0), s(1.0)));
    // Clip To Output Black / White: After Effects' default "Off for 32 bpc Color" (item 3 there,
    // item 2 here).
    assert_eq!((v(&g, "clipToOutputBlack"), v(&g, "clipToOutputWhite"), v(&g, "channel")), (Value::Enum(2), Value::Enum(2), Value::Enum(0)));
    assert_eq!(v(&only_effect("514-fx-levels-gamma-2.aep"), "gamma"), s(2.0));
    assert_eq!(v(&only_effect("516-fx-levels-outwhite-0.9.aep"), "outWhite"), s(f32r(0.9)));
}

#[test]
fn expression_controls() {
    assert_eq!(v(&only_effect("071-fx-slider-value77.aep"), "slider"), s(77.0));
    assert_eq!(v(&only_effect("304-fx-slider-0-explicit.aep"), "slider"), s(0.0));
    let k = only_effect("221-fx-slider-2key.aep").prop("slider").expect("slider").keys.clone();
    assert_eq!(k.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>(), vec![(Tick::ZERO, s(10.0)), (Tick(2 * TICKS_PER_SECOND), s(90.0))]);
    assert_eq!(v(&only_effect("217-fx-checkbox-on.aep"), "checkbox"), Value::Bool(true));
    let Value::Color(c) = v(&only_effect("073-fx-color-0.2-0.6-1.aep"), "color") else { panic!("colour") };
    for (a, b) in c.iter().zip([0.2, 0.6, 1.0, 1.0]) {
        assert!((a - b).abs() < 1e-6, "{c:?}");
    }
    let Value::Vec2(p) = v(&only_effect("218-fx-point-111-222.aep"), "point") else { panic!("point") };
    assert!((p[0] - 111.0).abs() < 1e-6 && (p[1] - 222.0).abs() < 1e-6, "{p:?}");
    assert_eq!(v(&only_effect("219-fx-angle-90.aep"), "angle"), s(90.0));
}

#[test]
fn fill() {
    let g = only_effect("215-fx-fill-default.aep");
    assert_eq!(g.match_id, "ec.generate.fill");
    assert_eq!(
        (v(&g, "color"), v(&g, "allMasks"), v(&g, "invert"), v(&g, "horizontalFeather"), v(&g, "verticalFeather")),
        (Value::Color([1.0, 0.0, 0.0, 1.0]), Value::Bool(false), Value::Bool(false), s(0.0), s(0.0))
    );
    close(&v(&g, "opacity"), 100.0);
    close(&v(&only_effect("303-fx-fill-opacity-50-then-100.aep"), "opacity"), 100.0);
}

/// Drops every `pdnm` (popup items) from the tree.
fn drop_pdnm(chunks: &mut Vec<effectcraft_aep::Chunk<'_>>) -> usize {
    let before = chunks.len();
    chunks.retain(|c| c.id != *b"pdnm");
    let mut n = before - chunks.len();
    for c in chunks.iter_mut() {
        if let effectcraft_aep::Body::List { children, .. } | effectcraft_aep::Body::Chunks(children) = &mut c.body {
            n += drop_pdnm(children);
        }
    }
    n
}

#[test]
fn popups_without_item_names_use_after_effects_items() {
    // A popup may come without its `pdnm`; the item count in `pard` still says how many
    // there are. 410 (Invert Channel Red) and 427 (Glow) with every `pdnm` removed.
    for (name, id, want) in
        [("410-fx-invert-channel-2.aep", "channel", 1), ("427-fx-glow-default.aep", "operation", 1), ("427-fx-glow-default.aep", "glowOperation", 1)]
    {
        let bytes = common::fixture_bytes(name);
        let mut file = effectcraft_aep::parse(&bytes).expect("parse");
        assert!(drop_pdnm(&mut file.chunks) > 0, "{name}");
        let bytes = effectcraft_aep::write(&file).expect("write");
        let mut p = Project::default();
        let none = |_: &str| false;
        let r = effectcraft_aep_import::import(&mut p, &bytes, &effectcraft_aep_import::ImportOptions::new(&none)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, c) = comp(&p, &r, "C");
        let l = c.layers.iter().find(|l| l.name == "L").expect("L");
        let g = l.props.sub("effects").and_then(|fx| fx.children.iter().find_map(|n| n.as_group())).expect("effect");
        assert_eq!(v(g, id), Value::Enum(want), "{name} {id}");
    }
}

#[test]
fn timewarp() {
    let g = only_effect("529-fx-timewarp-default.aep");
    assert_eq!(g.match_id, "ec.time.timewarp");
    // After Effects' defaults: Pixel Motion, by Speed, 50, Source Frame 0.
    assert_eq!((v(&g, "method"), v(&g, "adjustTimeBy"), v(&g, "speed"), v(&g, "sourceFrame")), (Value::Enum(2), Value::Enum(0), s(50.0), s(0.0)));
    assert_eq!(v(&only_effect("530-fx-timewarp-speed-25.aep"), "speed"), s(25.0));
    let k = only_effect("531-fx-timewarp-speed-keyed-50-200.aep").prop("speed").expect("speed").keys.clone();
    assert_eq!(k.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>(), vec![(Tick::ZERO, s(50.0)), (Tick(2 * TICKS_PER_SECOND), s(200.0))]);
    assert_eq!(v(&only_effect("532-fx-timewarp-method-1.aep"), "method"), Value::Enum(0));
    let g = only_effect("533-fx-timewarp-sourceframe-12.aep");
    assert_eq!((v(&g, "adjustTimeBy"), v(&g, "sourceFrame")), (Value::Enum(1), s(12.0)));
}

#[test]
fn an_expression_on_an_effect_parameter() {
    let g = only_effect("534-fx-slider-expression-time10.aep");
    let e = g.prop("slider").and_then(|p| p.expr.clone()).expect("expression");
    assert_eq!((e.text.as_str(), e.enabled), ("time*10", true));
}
