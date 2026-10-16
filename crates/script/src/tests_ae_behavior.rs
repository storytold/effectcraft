use std::sync::Arc;

use effectcraft_engine::project::ItemId;
use effectcraft_engine::{KeyRef, Session};
use serde_json::json;

fn run(s: &mut Session, code: &str) -> serde_json::Value {
    let out = crate::run_code(s, code, "ae-behavior.jsx");
    assert!(out.error.is_none(), "{:?}", out.error);
    out.result
}

fn fixture() -> (Session, ItemId, ItemId, KeyRef, KeyRef) {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    run(
        &mut s,
        r#"
        for (var i = 0; i < 2; i++) {
            var c = app.project.items.addComp(i ? "B" : "A", 64, 64, 1, 3, 30);
            var l = c.layers.addSolid([1,1,1], "S", 8, 8, 1);
            var p = l.property("ADBE Transform Group").property("ADBE Rotate Z");
            p.setValueAtTime(0, 0); p.setValueAtTime(1, 100); p.setValueAtTime(2, 200);
        }
    "#,
    );
    let key = |name| {
        let item = s.project.items.values().find(|i| i.name == name && i.as_comp().is_some()).unwrap();
        let l = &item.as_comp().unwrap().layers[0];
        let p = l.transform().unwrap().get("rotation").unwrap();
        (item.id, KeyRef { layer: l.id, prop: p.uid, time: p.keys[0].time })
    };
    let (a, ka) = key("A");
    let (b, kb) = key("B");
    s.activate_comp(b);
    s.state.selected_keys = vec![kb];
    s.activate_comp(a);
    s.state.selected_keys = vec![ka];
    (s, a, b, ka, kb)
}

const B_PROPERTY: &str = r#"
    var c; for(var i=1;i<=app.project.numItems;i++) if(app.project.item(i).name==="B") c=app.project.item(i);
    var p=c.layer(1).property("ADBE Transform Group").property("ADBE Rotate Z");
"#;

#[test]
fn inactive_comp_key_methods_preserve_both_selections() {
    let (mut s, a, b, ka, kb) = fixture();
    for (code, expected) in [
        ("p.setInterpolationTypeAtKey(1,KeyframeInterpolationType.HOLD); p.valueAtTime(0.5,true)", json!(0)),
        (
            "p.setTemporalEaseAtKey(2,[new KeyframeEase(12,45)],[new KeyframeEase(34,65)]); [p.keyInTemporalEase(2)[0].speed,p.keyOutTemporalEase(2)[0].influence]",
            json!([12, 65]),
        ),
        ("p.removeKey(3); p.numKeys", json!(2)),
    ] {
        assert_eq!(run(&mut s, &format!("{B_PROPERTY}{code}")), expected);
        assert_eq!(s.state.active_comp, Some(a));
        assert_eq!(s.state.selected_keys, vec![ka]);
        s.activate_comp(b);
        assert_eq!(s.state.selected_keys, vec![kb]);
        s.activate_comp(a);
    }
    run(&mut s, &format!("{B_PROPERTY}p.removeKey(1)"));
    assert_eq!(s.state.selected_keys, vec![ka]);
    s.activate_comp(b);
    assert!(s.state.selected_keys.is_empty(), "deleted target selection is discarded");
}

#[test]
fn inactive_comp_key_command_error_restores_selections() {
    let (mut s, a, b, ka, kb) = fixture();
    let code = format!("{B_PROPERTY}p.__keyCmd(1, 'no.such.command', {{}})");
    let out = crate::run_code(&mut s, &code, "error.jsx");
    assert!(out.error.is_some());
    assert_eq!(s.state.active_comp, Some(a));
    assert_eq!(s.state.selected_keys, vec![ka]);
    s.activate_comp(b);
    assert_eq!(s.state.selected_keys, vec![kb]);
}

#[test]
fn key_command_preserves_no_active_composition() {
    let (mut s, _, b, _, kb) = fixture();
    s.activate_comp_none();
    run(&mut s, &format!("{B_PROPERTY}p.setInterpolationTypeAtKey(1,KeyframeInterpolationType.HOLD)"));
    assert_eq!(s.state.active_comp, None);
    s.activate_comp(b);
    assert_eq!(s.state.selected_keys, vec![kb]);
}

#[test]
fn separated_position_leader_combines_pre_and_post_expression_followers() {
    let (mut s, _, _, _, _) = fixture();
    let result = run(
        &mut s,
        &format!(
            r#"{B_PROPERTY}
        p=c.layer(1).property("ADBE Transform Group").property("ADBE Position");
        p.dimensionsSeparated=true;
        var x=p.getSeparationFollower(0), y=p.getSeparationFollower(1);
        x.setValueAtTime(0,10); x.setValueAtTime(1,30); y.setValue(40);
        x.expression="value + 100"; y.expression="value + 7";
        var pre=p.valueAtTime(0.5,true), post=p.valueAtTime(0.5,false);
        [pre[0],pre[1],post[0],post[1],x.valueAtTime(0.5,false),y.valueAtTime(0.5,false)]
    "#
        ),
    );
    assert_eq!(result, json!([20, 40, 120, 47, 120, 47]));
}

#[test]
fn temporal_auto_bezier_enables_continuous_on_inactive_comp() {
    let (mut s, a, _, ka, _) = fixture();
    let result = run(
        &mut s,
        &format!(
            r#"{B_PROPERTY}
        p.setTemporalContinuousAtKey(2,false);
        p.setTemporalAutoBezierAtKey(2,true);
        [p.keyTemporalAutoBezier(2),p.keyTemporalContinuous(2)]
    "#
        ),
    );
    assert_eq!(result, json!([true, true]));
    assert_eq!(s.state.active_comp, Some(a));
    assert_eq!(s.state.selected_keys, vec![ka]);
}

#[test]
fn numeric_bezier_insertion_keeps_original_middle_speed_with_requested_value() {
    let (mut s, _, _, _, _) = fixture();
    let result = run(
        &mut s,
        &format!(
            r#"{B_PROPERTY}
        p.removeKey(2); p.setValueAtKey(2,100);
        for(var k=1;k<=2;k++) {{
            p.setInterpolationTypeAtKey(k,KeyframeInterpolationType.BEZIER,KeyframeInterpolationType.BEZIER);
            p.setTemporalAutoBezierAtKey(k,false);
            p.setTemporalContinuousAtKey(k,false);
            p.setTemporalEaseAtKey(k,[new KeyframeEase(0,100/3)],[new KeyframeEase(0,100/3)]);
        }}
        p.setValueAtTime(1,30);
        [p.keyValue(2),p.keyInTemporalEase(2)[0].speed,p.keyOutTemporalEase(2)[0].speed,p.keyTemporalContinuous(2)]
    "#
        ),
    );
    assert_eq!(result[0], json!(30));
    for index in [1, 2] {
        assert!((result[index].as_f64().unwrap() - 75.0).abs() < 1e-7, "{result}");
    }
    assert_eq!(result[3], json!(true));
}
