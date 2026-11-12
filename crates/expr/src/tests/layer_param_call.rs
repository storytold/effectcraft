//! A layer-valued property stands for its layer when called, as in After Effects:
//! `effect("Map")("Layer")("Effects")`, `kartonFX("Base Layer")("ADBE Effect Parade")(…)`. Rigs
//! (Karton's text boxes) read the base layer's controls that way; the call threw "not a callable
//! function" inside their `try`, so every box fell back to its stored position.

use super::*;

/// Layer B gets a Layer Control ("Map") pointing at layer A (which has a Slider Control, 42).
fn mapped() -> Fx {
    map_to(true)
}

/// [`mapped`], or with the Layer Control pointing at no layer.
fn map_to(a: bool) -> Fx {
    let mut f = fx();
    let spec = effectcraft_effects::find("ec.control.layer").unwrap();
    let mut map = group_push(&mut f.p, |ids| effectcraft_effects::instantiate(spec, ids, "Map", [20.0, 20.0]));
    map.prop_mut("layer").unwrap().value = Value::Layer(a.then_some(f.a.0));
    let b = f.b;
    f.layer_mut(b).props.sub_mut("effects").unwrap().children.push(map.into());
    f
}

#[test]
fn a_layer_valued_property_is_callable_like_its_layer() {
    let f = mapped();
    let on_b = |e: &str| f.ev(f.b, ROT, e, 0.0).map(|v| v.as_f64()).unwrap_or_else(|e| panic!("{e}"));
    assert_close!(on_b("effect('Map')('Layer')('Effects')('Slider Control')('Slider')"), 42.0);
    assert_close!(on_b("effect('Map')(1)('Effects').numProperties"), 1.0);
    assert_close!(on_b("var base = effect('Map')('Layer'); base('Transform')('Position')[0]"), 50.0);
    // What already worked keeps working.
    assert_close!(on_b("effect('Map')('Layer').effect('Slider Control')('Slider')"), 42.0);
    assert_close!(on_b("effect('Map')('Layer').name == 'A' ? 1 : 0"), 1.0);
}

#[test]
fn calling_another_property_is_still_a_type_error() {
    let f = mapped();
    let e = f.ev(f.a, ROT, "transform.position(1)", 0.0).unwrap_err();
    assert!(e.contains("not a callable function"), "{e}");
    // A Layer Control pointing at no layer: not callable either.
    let f = map_to(false);
    assert!(f.ev(f.b, ROT, "effect('Map')('Layer')('Effects')", 0.0).is_err());
}
