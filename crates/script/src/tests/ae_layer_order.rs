//! `layer.property(n)` and the layer's `numProperties` use After Effects' top-level numbers
//! (`project::ae_order`): Marker is 1 and unused slots keep their place as empty groups, so a loop
//! over `numProperties` meets every slot and never a null.

use std::sync::Arc;

use effectcraft_engine::Session;
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
fn layer_numbers_follow_after_effects() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var solid = c.layers.addSolid([1, 1, 1], "S", 200, 200, 1);
        solid.Effects.addProperty("ADBE Slider Control");
        var shape = c.layers.addShape();
        var text = c.layers.addText("t");
        var cam = c.layers.addCamera("Cam", [100, 100]);
        function names(l) { var o = []; for (var i = 1; i <= l.numProperties; i++) o.push(l.property(i).matchName); return o; }
        [names(solid), names(shape), names(text), names(cam),
         solid.property(5)(1)(1).value, solid.property(5).propertyIndex, solid.property("Effects").propertyIndex,
         solid.property(1) === solid.marker || solid.property(1).matchName, solid.property(16)]
        "#,
    );
    let av = [
        "ADBE Marker",
        "ADBE Time Remapping",
        "ADBE MTrackers",
        "ADBE Mask Parade",
        "ADBE Effect Parade",
        "ADBE Transform Group",
        "ADBE Layer Styles",
        "ADBE Plane Options Group",
        "ADBE Extrsn Options Group",
        "ADBE Material Options Group",
        "ADBE Audio Group",
        "ADBE Data Group",
        "ADBE Layer Overrides",
        "ADBE Layer Sets",
        "ADBE Source Options Group",
    ];
    let vector = |first: &str| {
        json!([
            "ADBE Marker",
            first,
            "ADBE Mask Parade",
            "ADBE Effect Parade",
            "ADBE Transform Group",
            "ADBE Layer Styles",
            "ADBE Extrsn Options Group",
            "ADBE Material Options Group",
            "ADBE Audio Group",
            "ADBE Layer Sets"
        ])
    };
    let r = o.result.as_array().unwrap();
    assert_eq!(r[0], json!(av), "solid");
    assert_eq!(r[1], vector("ADBE Root Vectors Group"), "shape");
    assert_eq!(r[2], vector("ADBE Text Properties"), "text");
    assert_eq!(r[3], json!(["ADBE Marker", "ADBE Transform Group", "ADBE Camera Options Group"]), "camera");
    assert_eq!(r[4], json!(0), "the slider's value through property(5)");
    assert_eq!(r[5], json!(5));
    assert_eq!(r[6], json!(5), "propertyIndex by name is After Effects' too");
    assert_eq!(r[7], json!("ADBE Marker"));
    assert_eq!(r[8], json!(null), "past the last slot");
}

#[test]
fn walking_every_property_meets_no_null() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var l = c.layers.addShape();
        var n = 0;
        function walk(g) {
          for (var i = 1; i <= g.numProperties; i++) {
            var p = g.property(i);
            if (p === null) throw new Error("null at " + i + " of " + g.name);
            if (p.propertyType === PropertyType.PROPERTY) n++; else walk(p);
          }
        }
        walk(l);
        n > 0
        "#,
    );
    assert_eq!(o.result, json!(true));
}

#[test]
fn an_unused_slot_is_the_same_by_number_and_match_name_and_callable() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var l = c.layers.addSolid([1, 1, 1], "S", 200, 200, 1);
        var byName = l.property("ADBE Layer Styles"), byNumber = l.property(7);
        [byName !== null && byName.matchName, byName !== null && byName.numProperties, byNumber.propertyIndex, l.property(7)(1), l(12).name]
        "#,
    );
    assert_eq!(o.result, json!(["ADBE Layer Styles", 0, 7, null, "Data"]));
}
