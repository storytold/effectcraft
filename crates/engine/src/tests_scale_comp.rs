//! Scale Composition (`comp.scale`, #2208): the comp and everything in it grow or shrink by one
//! factor, so the frame looks the same at the new size.

use effectcraft_project::{Layer, LayerId};
use effectcraft_time::Tick;
use serde_json::json;

use crate::{EngineError, Session};

fn comp(w: u32, h: u32) -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": w, "height": h, "frameRate": 10, "duration": 2})).unwrap();
    s
}

fn solid(s: &mut Session, color: &str, w: u32, h: u32) -> u64 {
    s.execute("layer.newSolid", json!({"color": color, "width": w, "height": h})).unwrap()["layer"].as_u64().unwrap()
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn v3(s: &Session, id: u64, path: &str, t: f64) -> [f64; 3] {
    layer(s, id).props.prop(path).unwrap().value_at(Tick::from_seconds_f64(t)).as_vec3()
}

/// Opaque pixels of `color_channel` (0 = red, 1 = green) at `t`: (x0, y0, x1, y1, count).
fn bounds(s: &Session, t: f64, channel: usize) -> (i64, i64, i64, i64, u32) {
    let img = s.render(s.active_comp_id().unwrap(), Tick::from_seconds_f64(t), Default::default());
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN, 0);
    for y in 0..img.height as i64 {
        for x in 0..img.width as i64 {
            let p = img.get(x, y);
            if p[3] > 0.5 && p[channel] > 0.5 {
                n += 1;
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    (x0, y0, x1, y1, n)
}

/// Twice as large on every side, from a pixel-exact frame.
fn doubled(b: (i64, i64, i64, i64, u32)) -> (i64, i64, i64, i64, u32) {
    (b.0 * 2, b.1 * 2, b.2 * 2 + 1, b.3 * 2 + 1, b.4 * 4)
}

#[test]
fn scale_comp_doubles_the_frame_with_keys_and_parented_layers() {
    let mut s = comp(200, 100);
    // A red solid moving right (two Position keys), and a green solid parented to a null.
    let red = solid(&mut s, "#ff0000", 20, 10);
    s.execute("prop.addKey", json!({"layer": red, "path": "transform/position", "time": 0, "value": [30, 20]})).unwrap();
    s.execute("prop.addKey", json!({"layer": red, "path": "transform/position", "time": 1, "value": [70, 20]})).unwrap();
    let green = solid(&mut s, "#00ff00", 10, 10);
    let rig = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": rig, "path": "transform/position", "value": [150, 60]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [green], "parent": rig})).unwrap();
    s.execute("prop.set", json!({"layer": green, "path": "transform/position", "value": [10, 15]})).unwrap();
    let before = s.project.clone();
    let (red0, red1, green0) = (bounds(&s, 0.0, 0), bounds(&s, 1.0, 0), bounds(&s, 0.0, 1));
    assert_eq!(red0, (20, 15, 39, 24, 200));

    let r = s.execute("comp.scale", json!({"factor": 2})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(400), Some(200)));
    let c = s.active_comp().unwrap();
    assert_eq!((c.width, c.height), (400, 200));
    // Every key moves, not only the value at the current time.
    assert_eq!(bounds(&s, 0.0, 0), doubled(red0));
    assert_eq!(bounds(&s, 1.0, 0), doubled(red1));
    assert_eq!(v3(&s, red, "transform/scale", 1.0), [200.0; 3]);
    // The child keeps its own transform: its scaled parent carries it.
    assert_eq!(bounds(&s, 0.0, 1), doubled(green0));
    assert_eq!(v3(&s, green, "transform/position", 0.0)[..2], [10.0, 15.0]);
    assert_eq!(v3(&s, green, "transform/scale", 0.0), [100.0; 3]);
    let after = s.project.clone();

    // One undo step, and one redo step.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project == before, "undo restores the comp and every layer");
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.project == after, "redo scales the comp and every layer again");
}

#[test]
fn scale_comp_to_a_width_keeps_the_aspect_and_scales_cameras_3d_and_guides() {
    let mut s = comp(640, 360);
    let card = solid(&mut s, "#ff0000", 100, 100);
    s.execute("layer.setSwitch", json!({"layers": [card], "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": card, "path": "transform/position", "value": [100, 50, 200]})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("view.addGuide", json!({"orientation": "vertical", "position": 160})).unwrap();
    s.execute("view.addGuide", json!({"orientation": "horizontal", "position": 25, "percent": true})).unwrap();
    let zoom = |s: &Session| layer(s, cam).props.prop("cameraOptions/zoom").unwrap().value.as_f64();
    let (zoom0, cam0, poi0) = (zoom(&s), v3(&s, cam, "transform/position", 0.0), v3(&s, cam, "transform/poi", 0.0));
    let seen0 = bounds(&s, 0.0, 0);
    s.execute("view.setRegionOfInterest", json!({"rect": [10, 20, 70, 70]})).unwrap();

    let r = s.execute("comp.scale", json!({"width": 960})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["factor"].as_f64()), (Some(960), Some(540), Some(1.5)));
    assert_eq!(v3(&s, card, "transform/position", 0.0), [150.0, 75.0, 300.0]);
    assert_eq!(v3(&s, card, "transform/scale", 0.0), [150.0; 3]);
    assert!((zoom(&s) - zoom0 * 1.5).abs() < 1e-9, "the camera sees the same frame");
    assert_eq!(v3(&s, cam, "transform/position", 0.0), cam0.map(|v| v * 1.5));
    assert_eq!(v3(&s, cam, "transform/poi", 0.0), poi0.map(|v| v * 1.5));
    // Through the camera the card covers the same part of the frame.
    let seen = bounds(&s, 0.0, 0);
    let near = |a: i64, b: i64| (a as f64 - b as f64 * 1.5).abs() <= 2.0;
    assert!(near(seen.0, seen0.0) && near(seen.1, seen0.1) && near(seen.2, seen0.2) && near(seen.3, seen0.3), "{seen0:?} → {seen:?}");
    let c = s.active_comp().unwrap();
    assert_eq!(c.guides[0].px(c.width, c.height), 240.0);
    assert_eq!(c.guides[1].position, 25.0, "a percentage guide keeps its percent");
    assert_eq!(s.state.region_of_interest, Some([15.0, 30.0, 105.0, 105.0]), "the region of interest covers the same part");

    // A height alone works the same way.
    let r = s.execute("comp.scale", json!({"height": 270})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(480), Some(270)));
}

#[test]
fn scale_comp_scales_depth_under_a_2d_parent_and_children_of_a_camera() {
    let mut s = comp(200, 100);
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    // A 3D card under a 2D null: the null's Scale carries X and Y but not Z.
    let card = solid(&mut s, "#ff0000", 20, 10);
    let rig = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": rig, "path": "transform/position", "value": [100, 50]})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [card], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [card], "parent": rig})).unwrap();
    s.execute("prop.set", json!({"layer": card, "path": "transform/position", "value": [-40, 0, 500]})).unwrap();
    // A 3D card parented to the camera, in front of it.
    let held = solid(&mut s, "#00ff00", 10, 10);
    s.execute("layer.setSwitch", json!({"layers": [held], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [held], "parent": cam})).unwrap();
    s.execute("prop.set", json!({"layer": held, "path": "transform/position", "value": [30, 0, 500]})).unwrap();
    let (card0, held0) = (bounds(&s, 0.0, 0), bounds(&s, 0.0, 1));
    let (held_pos0, held_scale0) = (v3(&s, held, "transform/position", 0.0), v3(&s, held, "transform/scale", 0.0));
    assert!(card0.4 > 0 && held0.4 > 0, "both cards are in view: {card0:?} {held0:?}");

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    let near = |a: (i64, i64, i64, i64, u32), b: (i64, i64, i64, i64, u32)| {
        [(a.0, b.0 * 2), (a.1, b.1 * 2), (a.2, b.2 * 2 + 1), (a.3, b.3 * 2 + 1)].iter().all(|&(x, y)| (x - y).abs() <= 2)
    };
    let (card1, held1) = (bounds(&s, 0.0, 0), bounds(&s, 0.0, 1));
    assert!(near(card1, card0), "the card under the 2D null doubles: {card0:?} → {card1:?}");
    assert_eq!(v3(&s, card, "transform/position", 0.0), [-40.0, 0.0, 1000.0]);
    assert!(near(held1, held0), "the card held by the camera doubles: {held0:?} → {held1:?}");
    assert_eq!(v3(&s, held, "transform/position", 0.0), held_pos0.map(|v| v * 2.0));
    assert_eq!(v3(&s, held, "transform/scale", 0.0), held_scale0.map(|v| v * 2.0));
}

#[test]
fn scale_comp_keeps_the_ease_of_depth_moves_under_a_2d_parent() {
    let mut s = comp(200, 100);
    let card = solid(&mut s, "#ff0000", 20, 10);
    let rig = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [card], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [card], "parent": rig})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/position", "time": 0, "value": [0, 0, 0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/position", "time": 1, "value": [0, 0, 100]})).unwrap();
    s.execute("keys.setEase", json!({"layer": card, "path": "transform/position", "time": 0, "side": "out", "speed": 100, "influence": 50})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/scale", "time": 0, "value": [100, 100, 100]})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/scale", "time": 1, "value": [100, 100, 300]})).unwrap();
    s.execute("keys.setEase", json!({"layer": card, "path": "transform/scale", "time": 0, "side": "out", "speed": 300, "influence": 50})).unwrap();
    let (pos0, scale0) = (v3(&s, card, "transform/position", 0.25), v3(&s, card, "transform/scale", 0.25));

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    // Twice the distance in the same time: the same ease, every frame twice as deep.
    let (pos, scale) = (v3(&s, card, "transform/position", 0.25), v3(&s, card, "transform/scale", 0.25));
    assert!((pos[2] - pos0[2] * 2.0).abs() < 1e-6, "{pos0:?} → {pos:?}");
    assert!((scale[2] - scale0[2] * 2.0).abs() < 1e-6, "{scale0:?} → {scale:?}");
    assert_eq!(scale[..2], scale0[..2]);
}

#[test]
fn scale_comp_gives_z_its_own_speed_when_a_scale_key_has_one_ease() {
    let mut s = comp(200, 100);
    let card = solid(&mut s, "#ff0000", 20, 10);
    let rig = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [card], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [card], "parent": rig})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/scale", "time": 0, "value": [100, 100, 100]})).unwrap();
    s.execute("prop.addKey", json!({"layer": card, "path": "transform/scale", "time": 1, "value": [100, 100, 300]})).unwrap();
    s.execute("keys.setEase", json!({"layer": card, "path": "transform/scale", "time": 0, "side": "out", "speed": 300, "influence": 50})).unwrap();
    // One ease for every dimension, as a project file may store it.
    let cid = s.active_comp_id().unwrap();
    let key =
        &mut std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(card)).unwrap().props.prop_mut("transform/scale").unwrap().keys
            [0];
    key.out_ease.truncate(1);
    let scale0 = v3(&s, card, "transform/scale", 0.25);

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    let scale = v3(&s, card, "transform/scale", 0.25);
    assert!((scale[2] - scale0[2] * 2.0).abs() < 1e-6, "{scale0:?} → {scale:?}");
    assert_eq!(scale[..2], scale0[..2], "X and Y keep the shared ease");
}

#[test]
fn scale_comp_scales_depth_once_through_a_3d_grandparent() {
    let mut s = comp(200, 100);
    s.execute("layer.newCamera", json!({})).unwrap();
    // card (3D) → A (2D null) → B (3D null): B's Scale already carries the depth.
    let card = solid(&mut s, "#ff0000", 20, 10);
    let a = s.execute("layer.newNull", json!({"name": "A"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newNull", json!({"name": "B"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [card, b], "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [100, 50, 0]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [a], "parent": b})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [0, 0]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [card], "parent": a})).unwrap();
    s.execute("prop.set", json!({"layer": card, "path": "transform/position", "value": [-40, 0, 500]})).unwrap();
    let card0 = bounds(&s, 0.0, 0);
    assert!(card0.4 > 0, "the card is in view");

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    assert_eq!(v3(&s, card, "transform/position", 0.0), [-40.0, 0.0, 500.0]);
    let card1 = bounds(&s, 0.0, 0);
    let near =
        [(card1.0, card0.0 * 2), (card1.1, card0.1 * 2), (card1.2, card0.2 * 2 + 1), (card1.3, card0.3 * 2 + 1)].iter().all(|&(x, y)| (x - y).abs() <= 2);
    assert!(near, "{card0:?} → {card1:?}");
}

#[test]
fn scale_comp_scales_once_through_a_camera_under_a_scaled_null() {
    let mut s = comp(200, 100);
    // solid → camera → B (3D null): B's Scale reaches the solid through the camera.
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newNull", json!({"name": "B"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [cam], "parent": b})).unwrap();
    let held = solid(&mut s, "#00ff00", 10, 10);
    s.execute("layer.setSwitch", json!({"layers": [held], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setParent", json!({"layers": [held], "parent": cam})).unwrap();
    s.execute("prop.set", json!({"layer": held, "path": "transform/position", "value": [30, 0, 500]})).unwrap();
    let held0 = bounds(&s, 0.0, 1);
    assert!(held0.4 > 0, "the solid is in view: {held0:?}");

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    assert_eq!(v3(&s, held, "transform/position", 0.0), [30.0, 0.0, 500.0]);
    assert_eq!(v3(&s, held, "transform/scale", 0.0), [100.0; 3]);
    let held1 = bounds(&s, 0.0, 1);
    let near =
        [(held1.0, held0.0 * 2), (held1.1, held0.1 * 2), (held1.2, held0.2 * 2 + 1), (held1.3, held0.3 * 2 + 1)].iter().all(|&(x, y)| (x - y).abs() <= 2);
    assert!(near, "{held0:?} → {held1:?}");
}

#[test]
fn scale_comp_scales_the_point_of_interest_depth_of_a_camera_under_a_2d_null() {
    let mut s = comp(200, 100);
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    let rig = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setParent", json!({"layers": [cam], "parent": rig})).unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "transform/position", "value": [10, 20, -1000]})).unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "transform/poi", "value": [10, 20, 200]})).unwrap();

    s.execute("comp.scale", json!({"factor": 2})).unwrap();
    // The 2D null carries X and Y; the camera's own Z doubles at both ends, so it looks the same way.
    assert_eq!(v3(&s, cam, "transform/position", 0.0), [10.0, 20.0, -2000.0]);
    assert_eq!(v3(&s, cam, "transform/poi", 0.0), [10.0, 20.0, 400.0]);
}

#[test]
fn scale_comp_refuses_a_bad_factor_and_changes_nothing() {
    let mut s = comp(640, 360);
    solid(&mut s, "#ff0000", 100, 100);
    let before = s.project.clone();
    let cases = [
        (json!({}), "pass `factor`"),
        (json!({"factor": 0}), "4 to 30,000 px"),
        (json!({"factor": -2}), "4 to 30,000 px"),
        (json!({"width": 0}), "4 to 30,000 px"),
        (json!({"factor": 100}), "4 to 30,000 px"),
        (json!({"factor": 0.001}), "4 to 30,000 px"),
    ];
    for (p, why) in cases {
        match s.execute("comp.scale", p.clone()) {
            Err(EngineError::BadParams { cmd, msg }) => assert!(cmd == "comp.scale" && msg.contains(why), "{p}: {msg}"),
            other => panic!("{p} is refused as bad parameters, got {other:?}"),
        }
        assert!(s.project == before, "{p} leaves the project alone");
    }
    s.execute("comp.scale", json!({"factor": 1})).unwrap();
}
