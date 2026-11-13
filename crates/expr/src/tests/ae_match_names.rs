//! Properties and groups found by their After Effects match names inside groups, as rigs write
//! them so they work in any UI language: `l("ADBE Transform Group")("ADBE Scale")`. Only a
//! layer's top-level groups were found by match name; the rest failed with "Property … not
//! found", inside rigs' `try`, so they fell back to stored values.

use super::*;

#[test]
fn transform_properties_by_match_name() {
    let f = fx();
    assert_close!(f.num("thisComp.layer('A')('ADBE Transform Group')('ADBE Scale')[0]", 0.0), 100.0);
    assert_close!(f.num("thisComp.layer('A')('ADBE Transform Group')('ADBE Position')[0]", 0.0), 50.0);
    assert_close!(f.num("thisComp.layer('A').transform('ADBE Opacity')", 0.0), 100.0);
}

#[test]
fn effects_and_parameters_by_match_name() {
    let f = fx();
    assert_close!(f.num("thisComp.layer('A')('ADBE Effect Parade')('ADBE Slider Control')('ADBE Slider Control-0001')", 0.0), 42.0);
    assert_close!(f.num("effect('ADBE Slider Control')('ADBE Slider Control-0001')", 0.0), 42.0);
}

#[test]
fn shape_contents_and_masks_by_match_name() {
    let f = fx();
    let rect =
        "thisComp.layer('Shape')('ADBE Root Vectors Group')('ADBE Vector Group')('ADBE Vectors Group')('ADBE Vector Shape - Rect')('ADBE Vector Rect Size')[0]";
    assert_close!(f.num(rect, 0.0), 60.0);
    assert_close!(f.num("thisComp.layer('A')('ADBE Mask Parade')('ADBE Mask Atom')('ADBE Mask Feather')[0]", 0.0), 0.0);
    // A name nothing has is still an error.
    assert_close!(f.num("try { thisComp.layer('A')('ADBE Transform Group')('ADBE Nothing'); 1 } catch (e) { -1 }", 0.0), -1.0);
}
