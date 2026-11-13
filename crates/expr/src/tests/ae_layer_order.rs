//! A layer's top-level numbers are After Effects' (`project::ae_order`): Marker is 1, and slots the
//! layer does not use (Time Remap, Motion Trackers, Layer Styles…) keep their place. Rigs that read
//! a palette by number, such as `comp("Palette").layer(1)(4)(n)(1)` on a shape layer, failed
//! silently before: `(4)` reached Transform, the expression's `try` fell back to the stored value and
//! every linked colour rendered near-black.

use super::*;
use effectcraft_project::Property;

/// The fixture with a Slider Control (value 7) on layer "Shape".
fn with_shape_slider() -> Fx {
    let mut f = fx();
    let spec = effectcraft_effects::find("ec.control.slider").unwrap();
    let mut slider = group_push(&mut f.p, |ids| effectcraft_effects::instantiate(spec, ids, "Swatch 1", [20.0, 20.0]));
    slider.prop_mut("slider").unwrap().value = Value::Scalar(7.0);
    f.layer_mut(f.shape).props.sub_mut("effects").unwrap().children.push(slider.into());
    f
}

fn yes(f: &Fx, cond: &str) -> f64 {
    f.num(&format!("({cond}) ? 1 : 0"), 0.0)
}

#[test]
fn effects_by_number_on_a_shape_layer_is_four() {
    let f = with_shape_slider();
    // The Ray Dynamic Color lookup: layer(1)(4)(swatch)(1).
    assert_close!(f.num("thisComp.layer('Shape')(4)(1)(1)", 0.0), 7.0);
    assert_close!(f.num("try { thisComp.layer('Shape')(4)(1)(1) } catch (e) { -1 }", 0.0), 7.0);
}

#[test]
fn effects_by_number_on_a_solid_is_five() {
    let f = fx();
    assert_close!(f.num("thisComp.layer('A')(5)(1)(1)", 0.0), 42.0);
    assert_close!(f.num("thisComp.layer('A')(5)('Slider Control')('Slider')", 0.0), 42.0);
}

#[test]
fn top_level_names_follow_after_effects() {
    let f = fx();
    assert_close!(yes(&f, "thisComp.layer('A')(4).name == 'Masks'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('A')(5).name == 'Effects'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('A')(6).name == 'Transform'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('Shape')(2).name == 'Contents'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('Shape')(5).name == 'Transform'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('Title')(2).name == 'Text'"), 1.0);
    assert_close!(yes(&f, "thisComp.layer('Title')(4).name == 'Effects'"), 1.0);
    assert_close!(f.num("thisComp.layer('A')(6)('Position')[0]", 0.0), 50.0);
}

#[test]
fn one_is_the_markers() {
    let f = fx();
    assert_close!(f.num("thisComp.layer('A')(1).numKeys", 0.0), 2.0);
    assert_close!(f.num("thisComp.layer('A')(1).key(2).time", 0.0), 4.0);
}

#[test]
fn an_unused_slot_is_an_empty_group() {
    let f = fx();
    // Layer Styles (7) and Data (12) exist in After Effects before they are used.
    assert_close!(yes(&f, "thisComp.layer('A')(7).name == 'Layer Styles'"), 1.0);
    assert_close!(f.num("thisComp.layer('A')(12).numProperties", 0.0), 0.0);
    assert_close!(f.num("try { thisComp.layer('A')(12)(1); 1 } catch (e) { -1 }", 0.0), -1.0);
    // Past the last slot there is nothing.
    assert_close!(f.num("try { thisComp.layer('A')(16); 1 } catch (e) { -1 }", 0.0), -1.0);
}

#[test]
fn property_index_of_a_top_level_group_is_after_effects() {
    let f = fx();
    assert_close!(f.num("thisComp.layer('A').effect('Slider Control').propertyGroup(1).propertyIndex", 0.0), 5.0);
    assert_close!(f.num("thisComp.layer('A')('Transform').propertyIndex", 0.0), 6.0);
    assert_close!(f.num("thisComp.layer('Shape')('Contents').propertyIndex", 0.0), 2.0);
}

#[test]
fn top_level_groups_by_after_effects_match_name() {
    // Karton's expressions: thisLayer("ADBE Effect Parade")("…").
    let f = with_shape_slider();
    assert_close!(f.num("thisComp.layer('A')('ADBE Effect Parade')('Slider Control')('Slider')", 0.0), 42.0);
    assert_close!(f.num("thisComp.layer('Shape')('ADBE Effect Parade')(1)(1)", 0.0), 7.0);
    assert_close!(f.num("thisComp.layer('A')('ADBE Transform Group')('Position')[0]", 0.0), 50.0);
    assert_close!(f.num("thisComp.layer('A')('ADBE Marker').numKeys", 0.0), 2.0);
    assert_close!(f.num("thisComp.layer('A')('ADBE Effect Parade').propertyIndex", 0.0), 5.0);
}

#[test]
fn an_enabled_time_remap_is_number_two() {
    // Enabled Time Remap is a property at the top of the tree; After Effects numbers it 2.
    let mut f = fx();
    let uid = f.p.next_id;
    f.p.next_id += 1;
    f.layer_mut(f.a).props.children.insert(0, Property::new(uid, "timeRemap", "Time Remap", Value::Scalar(0.0)).into());
    assert_close!(f.num("thisComp.layer('A')('Time Remap').propertyIndex", 0.0), 2.0);
    assert_close!(f.num("thisComp.layer('A')(2).propertyIndex", 0.0), 2.0);
}
