//! Masks on text and shape layers (705, 706) and the shape operators Merge Paths, Offset Paths and
//! Repeater (707-710, 731).

mod common;

use common::{comp, load};
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Project, PropGroup};

fn layer<'p>(p: &'p Project, r: &effectcraft_aep_import::ImportResult, name: &str) -> &'p Layer {
    let (_, c) = comp(p, r, "C");
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no {name}"))
}

fn mask_box(l: &Layer) -> [f64; 4] {
    let m = l.props.sub("masks").and_then(|m| m.groups().next()).expect("a mask");
    let Value::Path(p) = &m.prop("path").unwrap().value else { panic!() };
    p.vertices.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, v| [b[0].min(v[0]), b[1].min(v[1]), b[2].max(v[0]), b[3].max(v[1])])
}

#[test]
fn masks_on_text_and_shape_layers_are_in_layer_pixels() {
    let (p, r) = load("705-mask-on-text-rect-40-m70-160-10.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(mask_box(layer(&p, &r, "T")), [40.0, -70.0, 160.0, 10.0]);
    let (p, r) = load("706-mask-on-shape-rect-0-m150-100-150.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!(mask_box(layer(&p, &r, "S")), [0.0, -150.0, 100.0, 150.0]);
}

/// The items of the first group of shape layer S.
fn items(name: &str) -> Vec<PropGroup> {
    let (p, r) = load(name);
    assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
    let l = layer(&p, &r, "S");
    let g = l.props.sub("contents").and_then(|c| c.groups().next()).expect("group");
    g.sub("contents").expect("group contents").groups().cloned().collect()
}

fn find<'a>(items: &'a [PropGroup], id: &str) -> &'a PropGroup {
    items.iter().find(|g| g.match_id == id).unwrap_or_else(|| panic!("no {id}"))
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id}")).value.clone()
}

#[test]
fn shape_operators_arrive() {
    assert_eq!(v(find(&items("707-shape-merge-subtract-rect-ellipse.aep"), "merge"), "mode"), Value::Enum(2));
    assert_eq!(v(find(&items("708-shape-merge-intersect-rect-ellipse.aep"), "merge"), "mode"), Value::Enum(3));
    let i = items("709-shape-repeater-3-pos150-60-rot10-scale90-80-op40.aep");
    let r = find(&i, "repeater");
    assert_eq!((v(r, "copies"), v(r, "offset"), v(r, "composite")), (Value::Scalar(3.0), Value::Scalar(0.0), Value::Enum(0)));
    assert_eq!(
        ["transform/position", "transform/scale", "transform/rotation", "transform/startOpacity", "transform/endOpacity"].map(|id| v(r, id)),
        [Value::Vec2([150.0, 60.0]), Value::Vec2([90.0, 80.0]), Value::Scalar(10.0), Value::Scalar(100.0), Value::Scalar(40.0)]
    );
    let o = items("710-shape-offset-paths-20-round.aep");
    let o = find(&o, "offset");
    assert_eq!(
        (v(o, "amount"), v(o, "join"), v(o, "miter"), v(o, "copies"), v(o, "copyOffset")),
        (Value::Scalar(20.0), Value::Enum(1), Value::Scalar(4.0), Value::Scalar(1.0), Value::Scalar(0.0))
    );
}

/// After Effects offsets copy i by Amount x (i + Copy Offset) (826-828): 828 (Amount 20, Copies 3,
/// Copy Offset 1.5) draws rings at 30, 50, 70 px; EffectCraft's copy i is Amount x (i + 1 +
/// copyOffset), so copyOffset is 0.5.
#[test]
fn offset_paths_copy_offset_is_after_effects_minus_one() {
    let i = items("828-shape-offset-paths-20-copies3-copyoffset1.5-stroke.aep");
    let o = find(&i, "offset");
    assert_eq!((v(o, "amount"), v(o, "copies"), v(o, "copyOffset")), (Value::Scalar(20.0), Value::Scalar(3.0), Value::Scalar(0.5)));
    let rings: Vec<f64> = (0..3).map(|k| 20.0 * (f64::from(k) + 1.0 + 0.5)).collect();
    assert_eq!(rings, [30.0, 50.0, 70.0]);
}

#[test]
fn shape_operator_defaults_are_after_effects_ones() {
    let i = items("731-shape-merge-offset-repeater-defaults.aep");
    assert_eq!(v(find(&i, "merge"), "mode"), Value::Enum(0));
    let o = find(&i, "offset");
    assert_eq!((v(o, "amount"), v(o, "join"), v(o, "copyOffset")), (Value::Scalar(10.0), Value::Enum(0), Value::Scalar(0.0)));
    let r = find(&i, "repeater");
    assert_eq!((v(r, "copies"), v(r, "transform/position")), (Value::Scalar(1.0), Value::Vec2([0.0, 0.0])));
}

#[test]
fn parallel_and_ambient_lights_arrive() {
    use effectcraft_project::{LayerSource, LightKind};
    for (name, l, kind, intensity, colour) in [
        ("701-light-parallel-int80-color.aep", "P", LightKind::Parallel, 80.0, [1.0, 0.5, 0.25]),
        ("702-light-ambient-int50-color.aep", "A", LightKind::Ambient, 50.0, [0.25, 0.5, 1.0]),
    ] {
        let (p, r) = load(name);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        let light = layer(&p, &r, l);
        assert!(matches!(light.source, LayerSource::Light { kind: k } if k == kind), "{name}: {:?}", light.source);
        let o = light.props.sub("lightOptions").expect("light options");
        assert_eq!(v(o, "intensity"), Value::Scalar(intensity));
        let Value::Color(c) = v(o, "color") else { panic!() };
        assert!((0..3).all(|i| (c[i] - colour[i]).abs() < 1e-3), "{c:?}");
    }
}

#[test]
fn keyed_orientation_arrives() {
    let (p, r) = load("703-key-orientation-0-to-10-20-30-linear.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let o = layer(&p, &r, "L").props.prop("transform/orientation").unwrap();
    assert_eq!(o.keys.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), [Value::Vec3([0.0; 3]), Value::Vec3([10.0, 20.0, 30.0])]);
    assert_eq!(o.keys[1].time, effectcraft_time::Tick(2 * effectcraft_time::TICKS_PER_SECOND));
}
