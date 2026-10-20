//! Layers, property groups and properties are callable like After Effects (#758):
//! `layer("ADBE Effect Parade")`, `fxGroup(1)` and `fx("Slider")` are `.property(...)`.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use serde_json::json;

use crate::{Outcome, run_code};

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), expr_check: Some(effectcraft_expr::check_syntax), ..Default::default() };
    crate::install(&mut s);
    s
}

fn ok(s: &mut Session, code: &str) -> Outcome {
    let o = run_code(s, code, "test.jsx");
    assert!(o.error.is_none(), "script failed: {:?}\noutput: {:?}", o.error, o.output);
    o
}

#[test]
fn layers_groups_and_properties_are_callable() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var l = c.layers.addSolid([1, 1, 1], "S", 200, 200, 1);
        l("ADBE Effect Parade").addProperty("ADBE Slider Control");
        var fxGroup = l("Effects");
        var fx = fxGroup(1);
        fx("Slider").setValue(42);
        l("ADBE Mask Parade").addProperty("ADBE Mask Atom");
        var pos = l("ADBE Transform Group")("ADBE Position");
        [
          l("ADBE Effect Parade").matchName, fxGroup.numProperties, fx.matchName, fx.name,
          l("Effects")("Slider Control")("Slider").value, fxGroup(1)(1).value,
          l("Masks")(1).matchName, l("Masks")(1)("ADBE Mask Shape").name,
          pos.matchName, pos.name, pos.propertyIndex, pos.parentProperty("Opacity").value,
          pos.parentProperty.parentProperty(1).matchName === l.property(1).matchName,
          l("Nope") === null, fxGroup(9) === null, pos("anything") === null,
          l instanceof AVLayer, fxGroup instanceof PropertyGroup, pos instanceof Property, l.name,
          l.transform.position.matchName, l.Effects(1).matchName
        ]
        "#,
    );
    assert_eq!(
        o.result,
        json!([
            "ADBE Effect Parade",
            1,
            "ADBE Slider Control",
            "Slider Control",
            42,
            42,
            "ADBE Mask Atom",
            "Mask Path",
            "ADBE Position",
            "Position",
            2,
            100,
            true,
            true,
            true,
            true,
            true,
            true,
            true,
            "S",
            "ADBE Position",
            "ADBE Slider Control"
        ])
    );
    let c = s.project.items.values().find_map(|i| i.as_comp()).expect("comp");
    let l = c.layers.first().expect("layer");
    assert_eq!(l.props.prop("effects/#1/slider").map(|p| p.value.clone()), Some(KV::Scalar(42.0)));
}

#[test]
fn calling_without_a_name_is_a_script_error() {
    let mut s = session();
    for code in [
        "var c = app.project.items.addComp('M', 100, 100, 1, 1, 25); var l = c.layers.addNull(); l();",
        "var c = app.project.items.addComp('M', 100, 100, 1, 1, 25); var l = c.layers.addNull(); l('Transform')(null);",
        "var c = app.project.items.addComp('M', 100, 100, 1, 1, 25); var l = c.layers.addNull(); new l('Transform');",
    ] {
        let o = run_code(&mut s, code, "test.jsx");
        assert!(o.error.is_some(), "expected an error for {code}");
    }
    // A called layer still reports itself as a layer, and its JSON result is unchanged.
    let o = ok(&mut s, "var c = app.project.items.addComp('N', 100, 100, 1, 1, 25); var l = c.layers.addNull(); l.name = 'Ctl'; l");
    assert_eq!(o.result, json!({ "type": "[object AVLayer]", "name": "Ctl" }));
}

#[test]
fn app_preferences_save_to_disk_is_callable() {
    let mut s = session();
    let o = ok(&mut s, "app.preferences.saveToDisk(); app.preferences.reload(); [typeof app.preferences.saveToDisk, app.preferences.getPrefAsLong('a', 'b')]");
    assert_eq!(o.result, json!(["function", 0]));
}
