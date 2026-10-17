//! `layer.bounds`: layer geometry on synthetic layers (and helpers the other analysis tests share).

use serde_json::{Value, json};

use crate::Session;

/// A 400×300 comp, black background, 2 s at 25 fps.
pub(crate) fn comp() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 400, "height": 300, "frameRate": 25, "duration": 2})).unwrap();
    s
}

pub(crate) fn solid(s: &mut Session, name: &str, w: u32, h: u32, color: &str) -> u64 {
    s.execute("layer.newSolid", json!({"name": name, "width": w, "height": h, "color": color})).unwrap()["layer"].as_u64().unwrap()
}

fn set(s: &mut Session, layer: u64, path: &str, value: Value) {
    s.execute("prop.set", json!({"layer": layer, "path": path, "value": value})).unwrap();
}

fn bounds_of(s: &mut Session, layer: u64) -> Value {
    let r = s.execute("layer.bounds", json!({"layers": [layer]})).unwrap();
    r["layers"][0].clone()
}

fn close(v: &Value, want: &[f64]) {
    let got: Vec<f64> = v.as_array().unwrap_or_else(|| panic!("not an array: {v}")).iter().map(|x| x.as_f64().unwrap()).collect();
    assert_eq!(got.len(), want.len(), "{v}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-3, "got {got:?}, want {want:?}");
    }
}

#[test]
fn bounds_follow_position_rotation_scale_and_parents() {
    let mut s = comp();
    let l = solid(&mut s, "Box", 100, 50, "#ff0000");
    let b = bounds_of(&mut s, l);
    assert_eq!(b["visible"], true, "{b}");
    close(&b["bounds"], &[150.0, 125.0, 100.0, 50.0]);
    close(&b["center"], &[200.0, 150.0]);
    close(&b["anchorInComp"], &[200.0, 150.0]);
    assert_eq!(b["onScreen"], 1.0);
    assert_eq!(b["source"], json!({"left": 0.0, "top": 0.0, "width": 100.0, "height": 50.0}));

    // Half off the left edge.
    set(&mut s, l, "transform/position", json!([0.0, 150.0, 0.0]));
    let b = bounds_of(&mut s, l);
    close(&b["bounds"], &[-50.0, 125.0, 100.0, 50.0]);
    assert_eq!(b["onScreen"], 0.5);

    // Rotation 90°: the box stands up; corners rotate clockwise.
    set(&mut s, l, "transform/position", json!([200.0, 150.0, 0.0]));
    set(&mut s, l, "transform/rotation", json!(90.0));
    let b = bounds_of(&mut s, l);
    close(&b["bounds"], &[175.0, 100.0, 50.0, 100.0]);
    close(&b["corners"][0], &[225.0, 100.0]);
    // 45°: the AABB grows to the rotated rectangle's extent; the visible fraction stays 1.
    set(&mut s, l, "transform/rotation", json!(45.0));
    let b = bounds_of(&mut s, l);
    let e = 150.0 / 2f64.sqrt();
    close(&b["bounds"], &[200.0 - e / 2.0, 150.0 - e / 2.0, e, e]);

    // Scale.
    set(&mut s, l, "transform/rotation", json!(0.0));
    set(&mut s, l, "transform/scale", json!([200.0, 50.0, 100.0]));
    close(&bounds_of(&mut s, l)["bounds"], &[100.0, 137.5, 200.0, 25.0]);
    set(&mut s, l, "transform/scale", json!([100.0, 100.0, 100.0]));

    // Parenting: a null parent's move and scale carry the child.
    let n = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setParent", json!({"layers": [l], "parent": n})).unwrap();
    let before = bounds_of(&mut s, l)["bounds"].clone();
    close(&before, &[150.0, 125.0, 100.0, 50.0]);
    let np = s.execute("prop.get", json!({"layer": n, "path": "transform/position"})).unwrap()["value"].clone();
    let (nx, ny) = (np[0].as_f64().unwrap(), np[1].as_f64().unwrap());
    set(&mut s, n, "transform/position", json!([nx + 30.0, ny - 20.0, 0.0]));
    close(&bounds_of(&mut s, l)["bounds"], &[180.0, 105.0, 100.0, 50.0]);
    set(&mut s, n, "transform/scale", json!([50.0, 50.0, 100.0]));
    let b = bounds_of(&mut s, l);
    assert!((b["bounds"][2].as_f64().unwrap() - 50.0).abs() < 1e-3, "{b}");
    assert_eq!(b["parent"], n);
    // The null itself draws nothing.
    let nb = bounds_of(&mut s, n);
    assert_eq!(nb["visible"], false);
    assert!(nb["why"].to_string().contains("nulls"), "{nb}");
}

#[test]
fn bounds_of_3d_layers_project_through_the_camera() {
    let mut s = comp();
    let l = solid(&mut s, "Card", 100, 50, "#00ff00");
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    let b = bounds_of(&mut s, l);
    assert_eq!(b["threeD"], true);
    assert_eq!(b["projected"], true, "{b}");
    // At z = 0 the default camera shows the layer at its 2D size.
    close(&b["bounds"], &[150.0, 125.0, 100.0, 50.0]);
    // Pushed away from the camera it gets smaller around the comp centre.
    set(&mut s, l, "transform/position", json!([200.0, 150.0, 500.0]));
    let b = bounds_of(&mut s, l);
    let w = b["bounds"][2].as_f64().unwrap();
    assert!(w < 99.0 && w > 10.0, "{b}");
    close(&b["center"], &[200.0, 150.0]);
    // Behind the camera: not projectable.
    set(&mut s, l, "transform/position", json!([200.0, 150.0, -5000.0]));
    let b = bounds_of(&mut s, l);
    assert_eq!(b["projected"], false, "{b}");
    assert!(b["bounds"].is_null());
}

#[test]
fn visibility_onscreen_overlaps_and_pixels() {
    let mut s = comp();
    let a = solid(&mut s, "A", 100, 100, "#ff0000");
    let b = solid(&mut s, "B", 100, 100, "#0000ff");
    set(&mut s, a, "transform/position", json!([150.0, 150.0, 0.0]));
    set(&mut s, b, "transform/position", json!([200.0, 150.0, 0.0]));
    let r = s.execute("layer.bounds", json!({"overlaps": true})).unwrap();
    assert_eq!(r["layers"].as_array().unwrap().len(), 2);
    let o = &r["overlaps"][0];
    assert_eq!(o["area"], 5000.0, "{r}");
    assert_eq!(o["fraction"], json!([0.5, 0.5]));

    // Opacity 0 and out of the layer's span: not visible, with reasons.
    set(&mut s, a, "transform/opacity", json!(0.0));
    let ra = bounds_of(&mut s, a);
    assert_eq!(ra["visible"], false);
    assert!(ra["why"].to_string().contains("opacity"), "{ra}");
    s.execute("layer.timing", json!({"layers": [b], "out": 1.0})).unwrap();
    let rb = s.execute("layer.bounds", json!({"layers": ["B"], "time": 1.5})).unwrap();
    assert!(rb["layers"][0]["why"].to_string().contains("out point"), "{rb}");
    // Invisible layers don't count as overlapping.
    let r = s.execute("layer.bounds", json!({"overlaps": true, "time": 1.5})).unwrap();
    assert_eq!(r["overlaps"], json!([]));

    // Drawn pixels include an effect's spread.
    set(&mut s, a, "transform/opacity", json!(100.0));
    s.execute("effect.apply", json!({"layers": [a], "effect": "Gaussian Blur"})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/blurriness", "value": 20.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/repeatEdgePixels", "value": false})).ok();
    let r = s.execute("layer.bounds", json!({"layers": [a], "pixels": true})).unwrap();
    let (bx, px) = (&r["layers"][0]["bounds"], &r["layers"][0]["pixelBounds"]);
    assert!(px[2].as_f64().unwrap() >= bx[2].as_f64().unwrap(), "{r}");

    // Bad input.
    assert!(s.execute("layer.bounds", json!({"layers": ["Nope"]})).is_err());
    assert!(s.execute("layer.bounds", json!({"comp": "Nope"})).is_err());
    assert!(s.execute_checked("layer.bounds", json!({"bogus": 1})).is_err());
}
