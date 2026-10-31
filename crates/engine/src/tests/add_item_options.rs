//! `layer.addItem {in, out, anchor}` (#2026): place a layer's In / Out points and Anchor Point in
//! the same call, so one command does what addItem + layer.timing + prop.set did.

use effectcraft_project::{Layer, LayerId, LayerSource};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;

/// A 10 s, 30 fps comp with a 100 x 50 solid item; returns the session, the item id and the solid's first layer.
fn setup() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let l = s.execute_checked("layer.newSolid", json!({"name": "S", "color": "#fff", "width": 100, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let item = match &s.active_comp().unwrap().layer(LayerId(l)).unwrap().source {
        LayerSource::Solid { item } => item.0,
        other => panic!("not a solid: {other:?}"),
    };
    (s, item, l)
}

fn layer(s: &Session, id: u64) -> &Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap()
}

fn anchor(s: &Session, id: u64) -> [f64; 3] {
    layer(s, id).props.prop("transform/anchor").unwrap().value.as_vec3()
}

fn secs(t: Tick) -> f64 {
    t.seconds()
}

#[test]
fn in_out_and_anchor_in_one_call() {
    let (mut s, item, _) = setup();
    let id = s.execute_checked("layer.addItem", json!({"item": item, "time": 1.0, "in": 2.0, "out": 4.0, "anchor": [10.0, 20.0]})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    let l = layer(&s, id);
    assert_eq!((secs(l.start_time), secs(l.in_point), secs(l.out_point)), (1.0, 2.0, 4.0));
    assert_eq!(anchor(&s, id)[..2], [10.0, 20.0]);
}

#[test]
fn only_in_or_only_out_keeps_the_other_default() {
    let (mut s, item, _) = setup();
    let a = s.execute_checked("layer.addItem", json!({"item": item, "in": 3.0})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!((secs(layer(&s, a).in_point), secs(layer(&s, a).out_point)), (3.0, 10.0));
    let b = s.execute_checked("layer.addItem", json!({"item": item, "out": 5.0})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!((secs(layer(&s, b).in_point), secs(layer(&s, b).out_point)), (0.0, 5.0));
}

#[test]
fn omitted_params_keep_the_default_placement() {
    let (mut s, item, first) = setup();
    let before = anchor(&s, first);
    let id = s.execute_checked("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let l = layer(&s, id);
    assert_eq!((secs(l.in_point), secs(l.out_point)), (0.0, 10.0));
    assert_eq!(anchor(&s, id), before);
}

#[test]
fn invalid_values_are_errors_and_change_nothing() {
    let (mut s, item, _) = setup();
    let n = s.active_comp().unwrap().layers.len();
    let bad: [Value; 8] = [
        json!({"in": 5.0, "out": 5.0}),
        json!({"in": 6.0, "out": 2.0}),
        json!({"in": -1.0}),
        json!({"in": "soon"}),
        json!({"out": 1e300}),
        json!({"anchor": [1.0]}),
        json!({"anchor": [1.0, "x"]}),
        json!({"anchor": [1e300, 0.0]}),
    ];
    for extra in bad {
        let mut p = json!({"item": item});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(s.execute("layer.addItem", p.clone()).is_err(), "{p}");
    }
    assert_eq!(s.active_comp().unwrap().layers.len(), n);
}
