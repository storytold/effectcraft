//! `propertyGroup(countUp)` on properties and property groups, and a group's `propertyIndex`
//! (#1619): name-independent references up the property tree, as in After Effects.

use super::*;

const SCALE_1: &str = "contents/#1/transform/scale";
const SCALE_2: &str = "contents/#2/transform/scale";

/// The test comp with a second shape group "Rectangle 2" on layer "Shape", and "Rectangle 1"'s
/// Transform ▸ Scale keyed 0 → 100 over 0–1 s.
fn staggered() -> Fx {
    let mut f = fx();
    let g = group_push(&mut f.p, |ids| {
        let rect = build::shape_rect(ids, [20.0, 20.0], [0.0, 0.0], 0.0);
        build::shape_group(ids, "Rectangle 2", vec![rect])
    });
    let shape = f.layer_mut(f.shape);
    shape.props.sub_mut("contents").unwrap().children.push(g.into());
    shape.props.prop_mut(SCALE_1).unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec2([0.0, 0.0])), Keyframe::new(secs(1.0), Value::Vec2([100.0, 100.0]))];
    f
}

impl Fx {
    fn shape_num(&self, path: &str, text: &str) -> f64 {
        match self.ev(self.shape, path, &format!("var r = ({text}); [r, r]"), 0.0) {
            Ok(v) => v.as_vec2()[0],
            Err(e) => panic!("{text}: {e}"),
        }
    }
    fn shape_err(&self, path: &str, text: &str) -> String {
        match self.ev(self.shape, path, text, 0.0) {
            Ok(v) => panic!("{text}: expected an error, got {v:?}"),
            Err(e) => e,
        }
    }
}

#[test]
fn shape_group_stagger_from_the_adobe_community_answer() {
    let f = staggered();
    // thisProperty = Scale, propertyGroup(1) = Transform, propertyGroup(2) = the shape group.
    let text = "offset = .1;\nid = thisProperty.propertyGroup(2).propertyIndex;\ncontent(\"Rectangle 1\").transform.scale.valueAtTime(time-id*offset)";
    let v = f.ev(f.shape, SCALE_2, text, 0.5).unwrap().as_vec2();
    // "Rectangle 2" is the second group in Contents: Rectangle 1's scale 0.2 s earlier.
    assert_close!(v[0], 30.0);
    assert_close!(v[1], 30.0);
}

#[test]
fn property_group_walks_up_from_the_evaluated_property() {
    let f = staggered();
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(1).name == 'Transform' ? 1 : 0"), 1.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup().name == 'Transform' ? 1 : 0"), 1.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(2).name == 'Rectangle 2' ? 1 : 0"), 1.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(3).name == 'Contents' ? 1 : 0"), 1.0);
    assert_close!(f.shape_num(SCALE_1, "thisProperty.propertyGroup(2).propertyIndex"), 1.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(2).propertyIndex"), 2.0);
    // A shape group holds Blend Mode, Contents and Transform, in After Effects' order.
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(1).propertyIndex"), 3.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(2).numProperties"), 3.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(3).numProperties"), 2.0);
    // Groups walk on: Transform → group → Contents.
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(1).propertyGroup(2).name == 'Contents' ? 1 : 0"), 1.0);
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(1).propertyGroup(1).propertyIndex"), 2.0);
}

#[test]
fn property_group_reaches_the_layer() {
    let f = fx();
    assert_close!(f.num("thisProperty.propertyGroup(1).name == 'Transform' ? 1 : 0", 0.0), 1.0);
    assert_close!(f.num("thisProperty.propertyGroup(2).name == 'A' ? 1 : 0", 0.0), 1.0);
    assert_close!(f.num("thisProperty.propertyGroup(2).index", 0.0), 1.0);
    assert_close!(f.num("thisProperty.propertyGroup(1).propertyGroup(1).name == 'A' ? 1 : 0", 0.0), 1.0);
    // The layer it reaches is the same object as `thisLayer`.
    assert_close!(f.num("thisLayer('transform').propertyGroup(1) === thisLayer ? 1 : 0", 0.0), 1.0);
    assert_close!(f.num("thisProperty.propertyGroup(2) === thisLayer ? 1 : 0", 0.0), 1.0);
    let s = staggered();
    assert_close!(s.shape_num(SCALE_2, "thisProperty.propertyGroup(4).name == 'Shape' ? 1 : 0"), 1.0);
}

#[test]
fn property_group_of_an_effect_parameter_is_the_effect() {
    let f = fx();
    let path = "effects/#1/slider";
    let ev = |text: &str| f.ev(f.a, path, text, 0.0).unwrap().as_f64();
    assert_close!(ev("thisProperty.propertyGroup(1).name == 'Slider Control' ? 1 : 0"), 1.0);
    assert_close!(ev("thisProperty.propertyGroup(2).name == 'Effects' ? 1 : 0"), 1.0);
    // The name-independent rig: read the effect this parameter belongs to by its own name.
    assert_close!(ev("effect(thisProperty.propertyGroup(1).name)('Slider') + 1"), 43.0);
    assert_close!(ev("thisProperty.propertyGroup(1).propertyIndex"), 1.0);
}

#[test]
fn property_group_on_other_layers_properties_and_groups() {
    let f = staggered();
    // From layer B: another layer's property and group, reached by name.
    let ev = |text: &str| f.ev(f.b, ROT, text, 0.0).unwrap().as_f64();
    assert_close!(ev("thisComp.layer('A').effect(1)('Slider').propertyGroup(1).name == 'Slider Control' ? 1 : 0"), 1.0);
    assert_close!(ev("thisComp.layer('Shape').content('Rectangle 2').propertyIndex"), 2.0);
    assert_close!(ev("thisComp.layer('Shape').content('Rectangle 2').transform.scale.propertyGroup(2).propertyIndex"), 2.0);
    assert_close!(ev("thisComp.layer('Shape').content('Rectangle 1').content('Fill 1').propertyGroup(2).name == 'Rectangle 1' ? 1 : 0"), 1.0);
    assert_close!(ev("thisComp.layer('Shape').content('Rectangle 1').propertyGroup(2).name == 'Shape' ? 1 : 0"), 1.0);
    assert_close!(ev("thisComp.layer('A').effect('Slider Control').propertyIndex"), 1.0);
    assert_close!(ev("thisComp.layer('A').mask('Mask 1').propertyGroup(1).numProperties"), 1.0);
}

#[test]
fn property_group_out_of_range_is_an_error_not_a_crash() {
    let f = staggered();
    for text in [
        "thisProperty.propertyGroup(0)",
        "thisProperty.propertyGroup(-1)",
        "thisProperty.propertyGroup(5)",
        "thisProperty.propertyGroup(1e300)",
        "thisProperty.propertyGroup(NaN)",
        "thisProperty.propertyGroup('x')",
        "thisProperty.propertyGroup(1).propertyGroup(4)",
    ] {
        let e = f.shape_err(SCALE_2, text);
        assert!(e.contains("propertyGroup"), "{text}: {e}");
    }
    let e = f.err("thisProperty.propertyGroup(3)");
    assert!(e.contains("propertyGroup"), "{e}");
    // A fractional count rounds down, as an index does.
    assert_close!(f.shape_num(SCALE_2, "thisProperty.propertyGroup(2.7).propertyIndex"), 2.0);
}
