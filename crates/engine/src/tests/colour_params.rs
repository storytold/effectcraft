//! #3756: a colour parameter with a non-numeric channel (NaN arrives as JSON null) is refused
//! instead of silently defaulting the channel to 0; a given colour must be a hex string or
//! `[r, g, b]`. Absent, null or `false` ("no fill/stroke") keeps the caller's default.
//! `file.replaceWithSolid` on an existing solid changes its colour and keeps its size and pixel
//! aspect (the scripting colour setter).

use effectcraft_project::{ItemKind, LayerId, LayerSource, Solid};
use serde_json::json;

use crate::Session;

fn setup() -> Session {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 64, "height": 36, "frameRate": 24, "duration": 1})).unwrap();
    s
}

fn solid_of(s: &Session, l: u64) -> &Solid {
    let LayerSource::Solid { item } = &s.active_comp().unwrap().layer(LayerId(l)).unwrap().source else {
        panic!("not a solid layer");
    };
    let ItemKind::Solid(so) = &s.project.item(*item).unwrap().kind else { panic!("not a solid item") };
    so
}

fn new_solid(s: &mut Session, p: serde_json::Value) -> u64 {
    s.execute_checked("layer.newSolid", p).unwrap()["layer"].as_u64().unwrap()
}

#[test]
fn new_solid_refuses_a_non_numeric_channel() {
    let mut s = setup();
    assert!(s.execute_checked("layer.newSolid", json!({"color": [null, 0.5, 0.5]})).is_err());
    assert!(s.execute_checked("layer.newSolid", json!({"color": ["1", 0, 0]})).is_err());
    assert!(s.execute_checked("layer.newSolid", json!({"color": "#nohex"})).is_err());
    assert_eq!(s.active_comp().unwrap().layers.len(), 0);
    // Absent or null keeps the default; a valid colour still works.
    let a = new_solid(&mut s, json!({}));
    let b = new_solid(&mut s, json!({"color": null}));
    let c = new_solid(&mut s, json!({"color": [0, 1, 0]}));
    assert_eq!(solid_of(&s, a).color, [0.85, 0.2, 0.2]);
    assert_eq!(solid_of(&s, b).color, [0.85, 0.2, 0.2]);
    assert_eq!(solid_of(&s, c).color, [0.0, 1.0, 0.0]);
}

#[test]
fn layer_settings_refuses_a_non_numeric_channel() {
    let mut s = setup();
    let l = new_solid(&mut s, json!({"color": [1, 0, 0]}));
    assert!(s.execute_checked("layer.settings", json!({"layer": l, "color": [null, 0, 0]})).is_err());
    assert_eq!(solid_of(&s, l).color, [1.0, 0.0, 0.0]);
    s.execute_checked("layer.settings", json!({"layer": l, "color": [0, 0, 1]})).unwrap();
    assert_eq!(solid_of(&s, l).color, [0.0, 0.0, 1.0]);
}

#[test]
fn import_solid_refuses_a_non_numeric_channel() {
    let mut s = setup();
    assert!(s.execute_checked("file.importSolid", json!({"color": [0, null, 0]})).is_err());
    s.execute_checked("file.importSolid", json!({"color": [0, 0, 1]})).unwrap();
}

#[test]
fn replace_with_solid_sets_colour_and_keeps_size_and_pixel_aspect() {
    let mut s = setup();
    let l = new_solid(&mut s, json!({"color": [1, 0, 0], "width": 100, "height": 50, "pixelAspect": 2.0}));
    let item = match &s.active_comp().unwrap().layer(LayerId(l)).unwrap().source {
        LayerSource::Solid { item } => item.0,
        other => panic!("not a solid: {other:?}"),
    };
    assert!(s.execute_checked("file.replaceWithSolid", json!({"item": item, "color": [null, 0, 0]})).is_err());
    assert_eq!(solid_of(&s, l).color, [1.0, 0.0, 0.0]);
    s.execute_checked("file.replaceWithSolid", json!({"item": item, "color": [0, 1, 0]})).unwrap();
    let so = solid_of(&s, l);
    assert_eq!(so.color, [0.0, 1.0, 0.0]);
    assert_eq!((so.width, so.height), (100, 50));
    assert_eq!(so.pixel_aspect, 2.0);
}

#[test]
fn shape_paint_and_light_colour_refuse_non_numeric_channels() {
    let mut s = setup();
    let shape = s.execute_checked("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute_checked("shape.fillStroke", json!({"layer": shape, "fill": [null, 0, 0]})).is_err());
    assert!(s.execute_checked("layer.newLight", json!({"kind": "Point", "color": [null, 0, 0]})).is_err());
    // Valid colours still apply, and `false` still means no fill.
    s.execute_checked("shape.fillStroke", json!({"layer": shape, "fill": [1, 0, 0]})).unwrap();
    s.execute_checked("shape.fillStroke", json!({"layer": shape, "fill": false})).unwrap();
    s.execute_checked("layer.newLight", json!({"kind": "Point", "color": [0, 1, 0]})).unwrap();
}
