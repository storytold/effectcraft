//! Effects: Tint, Gradient Ramp and Exposure arrive with every parameter and key in EffectCraft's
//! units (cases 356-397, the values each case was made with), in stack order; other effects and
//! parameters are reported.

mod common;

use common::{comp, fixture_bytes, load, try_import};
use effectcraft_aep_import::{LossCategory, analyze, supported_effects};
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{GroupKind, Layer, Project, PropGroup};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

/// The layer named `name` in composition `C`.
fn layer<'p>(project: &'p Project, r: &effectcraft_aep_import::ImportResult, name: &str) -> &'p Layer {
    let (_, c) = comp(project, r, "C");
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

/// The effect instances of a layer, in stack order.
fn effects(l: &Layer) -> Vec<&PropGroup> {
    l.props.sub("effects").expect("an effects group").groups().collect()
}

/// The only effect of `L` in a fixture imported without losses.
fn only_effect(name: &str) -> PropGroup {
    let mut project = Project::default();
    let r = try_import(&mut project, name, false).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
    assert_eq!(r.report.imported.effects, 1, "{name}");
    let fx = effects(layer(&project, &r, "L"));
    assert_eq!(fx.len(), 1, "{name}");
    fx[0].clone()
}

/// A parameter by its id; grouped ones by path (`master/exposure`).
fn value(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no parameter {id}")).value.clone()
}

fn assert_scalar(v: Value, want: f64, tol: f64) {
    let Value::Scalar(x) = v else { panic!("{v:?} is not a number") };
    assert!((x - want).abs() <= tol, "{x} != {want}");
}

fn assert_color(v: Value, want: [f64; 4], tol: f64) {
    let Value::Color(c) = v else { panic!("{v:?} is not a colour") };
    for (g, w) in c.iter().zip(want) {
        assert!((g - w).abs() <= tol, "{c:?} != {want:?}");
    }
}

fn assert_vec2(v: Value, want: [f64; 2]) {
    let Value::Vec2(p) = v else { panic!("{v:?} is not a point") };
    for (g, w) in p.iter().zip(want) {
        assert!((g - w).abs() <= 1e-9, "{p:?} != {want:?}");
    }
}

const T1: Tick = Tick(TICKS_PER_SECOND);
const T2: Tick = Tick(2 * TICKS_PER_SECOND);

#[test]
fn the_table_names_tint_gradient_ramp_and_exposure() {
    let t: Vec<_> = supported_effects().collect();
    for want in [
        ("ADBE Tint", "ec.color.tint"),
        ("ADBE Ramp", "ec.generate.gradientramp"),
        ("ADBE Exposure2", "ec.color.exposure"),
        ("ADBE Glo2", "ec.stylize.glow"),
        ("ADBE Easy Levels2", "ec.color.levels"),
        ("ADBE Timewarp", "ec.time.timewarp"),
    ] {
        assert!(t.contains(&want), "{want:?}");
    }
    assert_eq!(t.len(), 57);
    for (_, id) in t {
        assert!(effectcraft_effects::find(id).is_some(), "{id}");
    }
}

// ---------------------------------------------------------------------------- Tint

#[test]
fn default_tint_equals_effectcrafts_default() {
    // 356: every parameter at its default (alpha 0 in the file's defaults; the colours arrive opaque).
    let g = only_effect("356-fx-tint-default.aep");
    assert_eq!((g.match_id.as_str(), g.name.as_str()), ("ec.color.tint", "Tint"));
    assert!(matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.color.tint"));
    assert!(g.enabled);
    assert_eq!(value(&g, "black"), Value::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(value(&g, "white"), Value::Color([1.0, 1.0, 1.0, 1.0]));
    assert_eq!(value(&g, "amount"), Value::Scalar(100.0));
    // The file's pard defaults, converted, are EffectCraft's defaults.
    let spec = effectcraft_effects::find("ec.color.tint").unwrap();
    for ps in &spec.params {
        assert_eq!(value(&g, ps.id), effectcraft_effects::default_value(ps, [1920.0, 1080.0]), "{}", ps.id);
    }
}

#[test]
fn tint_values() {
    // 357: Map Black To [0.2, 0.4, 0.6] (float32 in the file).
    let g = only_effect("357-fx-tint-black-0.2-0.4-0.6.aep");
    assert_color(value(&g, "black"), [0.2, 0.4, 0.6, 1.0], 1e-7);
    assert_eq!(value(&g, "white"), Value::Color([1.0, 1.0, 1.0, 1.0]));
    // 361: Amount to Tint 0.
    assert_eq!(value(&only_effect("361-fx-tint-amount-0.aep"), "amount"), Value::Scalar(0.0));
}

#[test]
fn tint_keyframes() {
    // 363: Amount to Tint 20 at 0 s, 80 at 2 s, linear; After Effects reads 50 at 1 s.
    let g = only_effect("363-fx-tint-amount-2key.aep");
    let p = g.get("amount").unwrap();
    assert_eq!(p.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>(), [(Tick::ZERO, Value::Scalar(20.0)), (T2, Value::Scalar(80.0))]);
    assert!(p.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
    assert_eq!(p.value_at(T1), Value::Scalar(50.0));

    // 364: Map White To [1,1,0] at 0 s, [0,0,1] at 2 s, linear; [0.5,0.5,0.5] at 1 s.
    let g = only_effect("364-fx-tint-white-2key.aep");
    let p = g.get("white").unwrap();
    assert_eq!(p.keys.len(), 2);
    assert_eq!((p.keys[0].time, p.keys[1].time), (Tick::ZERO, T2));
    assert_eq!(p.keys[0].value, Value::Color([1.0, 1.0, 0.0, 1.0]));
    assert_eq!(p.keys[1].value, Value::Color([0.0, 0.0, 1.0, 1.0]));
    assert_color(p.value_at(T1), [0.5, 0.5, 0.5, 1.0], 1e-9);
}

#[test]
fn stack_order_is_kept_and_other_effects_are_reported() {
    // 366: Gaussian Blur above Tint: both arrive, in stack order.
    let (project, r) = load("366-fx-blur-then-tint.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.imported.effects, 2);
    let fx = effects(layer(&project, &r, "L"));
    assert_eq!(fx.iter().map(|g| g.match_id.as_str()).collect::<Vec<_>>(), ["ec.blur.gaussian", "ec.color.tint"]);
}

// ---------------------------------------------------------------------------- Gradient Ramp

#[test]
fn default_ramp_equals_effectcrafts_default() {
    // 368: Start [960, 0], End [960, 1080] on the 1920x1080 solid, black to white, linear.
    let g = only_effect("368-fx-ramp-default.aep");
    assert_eq!((g.match_id.as_str(), g.name.as_str()), ("ec.generate.gradientramp", "Gradient Ramp"));
    assert_eq!(value(&g, "start"), Value::Vec2([960.0, 0.0]));
    assert_eq!(value(&g, "end"), Value::Vec2([960.0, 1080.0]));
    assert_eq!(value(&g, "startColor"), Value::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(value(&g, "endColor"), Value::Color([1.0, 1.0, 1.0, 1.0]));
    assert_eq!(value(&g, "shape"), Value::Enum(0));
    assert_eq!(value(&g, "scatter"), Value::Scalar(0.0));
    assert_eq!(value(&g, "blend"), Value::Scalar(0.0));
    let spec = effectcraft_effects::find("ec.generate.gradientramp").unwrap();
    for ps in &spec.params {
        assert_eq!(value(&g, ps.id), effectcraft_effects::default_value(ps, [1920.0, 1080.0]), "{}", ps.id);
    }
}

#[test]
fn ramp_values() {
    assert_vec2(value(&only_effect("369-fx-ramp-start-100-200.aep"), "start"), [100.0, 200.0]);
    assert_eq!(value(&only_effect("371-fx-ramp-startcolor-red.aep"), "startColor"), Value::Color([1.0, 0.0, 0.0, 1.0]));
    // Outside the layer stays outside.
    assert_vec2(value(&only_effect("374-fx-ramp-end-2000-m100.aep"), "end"), [2000.0, -100.0]);
    // Radial Ramp is item 2 in the file, 1 here.
    assert_eq!(value(&only_effect("377-fx-ramp-shape-radial.aep"), "shape"), Value::Enum(1));
    // Scatter in pixels; Blend With Original 0.25 in the file is 25 % here.
    assert_eq!(value(&only_effect("378-fx-ramp-scatter-12.aep"), "scatter"), Value::Scalar(12.0));
    assert_eq!(value(&only_effect("380-fx-ramp-blend-25pct.aep"), "blend"), Value::Scalar(25.0));
}

#[test]
fn ramp_point_keyframes() {
    // 382: Start of Ramp [100,100] at 0 s, [1800,900] at 2 s, linear; [950,500] at 1 s.
    let g = only_effect("382-fx-ramp-start-2key.aep");
    let p = g.get("start").unwrap();
    assert_eq!(p.keys.len(), 2);
    assert_eq!((p.keys[0].time, p.keys[1].time), (Tick::ZERO, T2));
    assert_vec2(p.keys[0].value.clone(), [100.0, 100.0]);
    assert_vec2(p.keys[1].value.clone(), [1800.0, 900.0]);
    assert!(p.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
    assert_vec2(p.value_at(T1), [950.0, 500.0]);
    assert_eq!(p.keys[0].spatial_in, [0.0; 3]);
}

#[test]
fn second_instance_on_another_layer() {
    // 383: L keeps its default Ramp (its parT is empty); S100 (100x100) has Start [10,20].
    let mut project = Project::default();
    let r = try_import(&mut project, "383-fx-ramp-on-100x100-start-10-20.aep", false).unwrap();
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(r.report.imported.effects, 2);
    let small = effects(layer(&project, &r, "S100"));
    assert_eq!(small.len(), 1);
    assert_eq!(value(small[0], "start"), Value::Vec2([10.0, 20.0]));
    // Its default End is relative to its own size: [50, 100].
    assert_eq!(value(small[0], "end"), Value::Vec2([50.0, 100.0]));
    // L: the defaults on 1920x1080, not S100's values from the project-level copy.
    let l = effects(layer(&project, &r, "L"));
    assert_eq!(l.len(), 1);
    assert_eq!(value(l[0], "start"), Value::Vec2([960.0, 0.0]));
    assert_eq!(value(l[0], "end"), Value::Vec2([960.0, 1080.0]));
    assert_eq!(value(l[0], "shape"), Value::Enum(0));
    assert_eq!(value(l[0], "startColor"), Value::Color([0.0, 0.0, 0.0, 1.0]));
}

// ---------------------------------------------------------------------------- Exposure

/// Exposure's slider ids per group, in the file's order (Exposure, Offset, Gamma Correction).
const EXPOSURE_GROUPS: [[&str; 3]; 4] = [
    ["master/exposure", "master/offset", "master/gamma"],
    ["red/redExposure", "red/redOffset", "red/redGamma"],
    ["green/greenExposure", "green/greenOffset", "green/greenGamma"],
    ["blue/blueExposure", "blue/blueOffset", "blue/blueGamma"],
];

/// Float sliders hold the float32 value (0.1 is 0.10000000149).
const F32: f64 = 1e-6;

#[test]
fn default_exposure_equals_effectcrafts_default() {
    // 389: every parameter at its default; the group markers carry nothing (no losses).
    let g = only_effect("389-fx-exposure-default.aep");
    assert_eq!((g.match_id.as_str(), g.name.as_str()), ("ec.color.exposure", "Exposure"));
    assert_eq!(value(&g, "channels"), Value::Enum(0));
    for ids in EXPOSURE_GROUPS {
        assert_eq!([value(&g, ids[0]), value(&g, ids[1]), value(&g, ids[2])], [Value::Scalar(0.0), Value::Scalar(0.0), Value::Scalar(1.0)]);
    }
    assert_eq!(value(&g, "bypassLinearLight"), Value::Bool(false));
    let spec = effectcraft_effects::find("ec.color.exposure").unwrap();
    for ps in &spec.params {
        assert_eq!(value(&g, ps.id), effectcraft_effects::default_value(ps, [1920.0, 1080.0]), "{}", ps.id);
    }
}

#[test]
fn exposure_values() {
    // 390: Channels -> Individual Channels (2 in the file, 1 here).
    assert_eq!(value(&only_effect("390-fx-exposure-channels-individual.aep"), "channels"), Value::Enum(1));
    // 391: Exposure 1.5 stops, the rest default.
    let g = only_effect("391-fx-exposure-master-1.5.aep");
    assert_eq!(value(&g, "master/exposure"), Value::Scalar(1.5));
    assert_eq!((value(&g, "master/offset"), value(&g, "master/gamma"), value(&g, "channels")), (Value::Scalar(0.0), Value::Scalar(1.0), Value::Enum(0)));
    // 393: Offset 0.1; 394: Gamma Correction 2.2 (float32 in the file, imported as stored).
    assert_scalar(value(&only_effect("393-fx-exposure-offset-0.1.aep"), "master/offset"), 0.1, F32);
    assert_scalar(value(&only_effect("394-fx-exposure-gamma-2.2.aep"), "master/gamma"), 2.2, F32);
    // 395: Individual Channels and Red Exposure 1; Master and the other channels stay default.
    let g = only_effect("395-fx-exposure-individual-red-1.aep");
    assert_eq!(value(&g, "channels"), Value::Enum(1));
    assert_eq!(value(&g, "red/redExposure"), Value::Scalar(1.0));
    assert_eq!(
        (value(&g, "master/exposure"), value(&g, "green/greenExposure"), value(&g, "blue/blueExposure")),
        (Value::Scalar(0.0), Value::Scalar(0.0), Value::Scalar(0.0))
    );
    // 396: Bypass Linear Light Conversion on.
    let g = only_effect("396-fx-exposure-bypass-on.aep");
    assert_eq!(value(&g, "bypassLinearLight"), Value::Bool(true));
    assert_eq!(value(&g, "master/exposure"), Value::Scalar(0.0));
}

#[test]
fn exposure_keyframes() {
    // 397: Exposure -1 at 0 s, 2 at 2 s, linear; After Effects reads 0.5 at 1 s.
    let g = only_effect("397-fx-exposure-master-2key.aep");
    let p = g.prop("master/exposure").unwrap();
    assert_eq!(p.keys.iter().map(|k| (k.time, k.value.clone())).collect::<Vec<_>>(), [(Tick::ZERO, Value::Scalar(-1.0)), (T2, Value::Scalar(2.0))]);
    assert!(p.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
    assert_eq!(p.value_at(T1), Value::Scalar(0.5));
    // The other sliders are not keyed.
    assert!(g.prop("master/offset").unwrap().keys.is_empty());
}

#[test]
fn unmapped_effects_stay_reported_by_match_name() {
    // A dropdown is not in the table (a Layer Control is since 905).
    {
        let (name, matchname) = ("077-fx-dropdown-default.aep", "Pseudo/");
        let report = analyze(&fixture_bytes(name), &|_| false).unwrap();
        assert_eq!(report.count(LossCategory::Effects), 1, "{name}");
        assert_eq!(report.imported.effects, 0);
        assert!(report.losses[0].details[0].contains(matchname), "{:?}", report.losses);
    }
}

#[test]
fn imported_effects_survive_a_json_round_trip() {
    let (project, r) = load("383-fx-ramp-on-100x100-start-10-20.aep");
    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(effects(layer(&back, &r, "S100"))[0].get("start").unwrap().value, Value::Vec2([10.0, 20.0]));
    let report = serde_json::to_value(&r.report).unwrap();
    assert_eq!(report["imported"]["effects"], 2);
}

#[test]
fn a_renamed_effect_keeps_its_name() {
    // 610: a Tint added with the name My Tint (fnam stays Tint, the name is in the value group).
    assert_eq!(only_effect("610-fx-tint-named-My-Tint.aep").name, "My Tint");
    assert_eq!(only_effect("356-fx-tint-default.aep").name, "Tint");
}
