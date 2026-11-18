//! `ParagraphJustification` has After Effects' numbers (26.3): MULTIPLE_JUSTIFICATIONS 7412,
//! LEFT 7413, RIGHT 7414, CENTER 7415, then the justified variants with the last line left (7416),
//! right (7417), centred (7418) and full (7419). A script that stores or compares the numbers gets
//! the same paragraph in both applications.

use serde_json::json;

use super::{comp_named, ok, session};
use effectcraft_engine::keyframe::{Justify, Value as KV};

#[test]
fn the_numbers_are_after_effects() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var P = ParagraphJustification;
        [P.MULTIPLE_JUSTIFICATIONS, P.LEFT_JUSTIFY, P.RIGHT_JUSTIFY, P.CENTER_JUSTIFY, P.FULL_JUSTIFY_LASTLINE_LEFT,
         P.FULL_JUSTIFY_LASTLINE_RIGHT, P.FULL_JUSTIFY_LASTLINE_CENTER, P.FULL_JUSTIFY_LASTLINE_FULL]
        "#,
    );
    assert_eq!(o.result, json!([7412, 7413, 7414, 7415, 7416, 7417, 7418, 7419]));
}

#[test]
fn each_number_sets_and_reads_its_paragraph() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("J", 200, 100, 1, 2, 30);
        var t = c.layers.addText("ab");
        var st = t.property("ADBE Text Properties").property("ADBE Text Document");
        var back = [];
        for (var n = 7413; n <= 7419; n++) {
          var d = st.value; d.justification = n; st.setValue(d);
          back.push(st.value.justification);
        }
        var d = st.value; d.justification = 7414; st.setValue(d);
        back
        "#,
    );
    assert_eq!(o.result, json!([7413, 7414, 7415, 7416, 7417, 7418, 7419]));
    // 7414 is right-aligned, as in After Effects.
    let (_, c) = comp_named(&s, "J");
    let KV::Text(d) = &c.layers[0].props.prop("text/sourceText").unwrap().value else { panic!("not text") };
    assert_eq!(d.justify, Justify::Right);
}
