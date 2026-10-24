//! Tests for the menu-parity command families (Layer / Edit / Animation / File / Composition /
//! View menus), including undo.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{FrameBlend, ItemKind, MatteKind, Quality, Sampling};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::{EngineError, Event, Session};

fn comp() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s
}

fn solid(s: &mut Session, color: &str) -> u64 {
    s.execute("layer.newSolid", json!({"color": color, "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap()
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(effectcraft_project::LayerId(id)).unwrap().clone()
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-menu-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

fn frontend_events(s: &mut Session) -> Vec<(String, Value)> {
    s.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Frontend { command, params } => Some((command, params)),
            _ => None,
        })
        .collect()
}

#[test]
fn layer_quality_sampling_frame_blending_and_undo() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("layer.quality", json!({"quality": "wireframe"})).unwrap();
    s.execute("layer.sampling", json!({"sampling": "bicubic"})).unwrap();
    s.execute("layer.frameBlending", json!({"mode": "pixelMotion"})).unwrap();
    let l = layer(&s, a);
    assert_eq!((l.switches.quality, l.switches.sampling, l.switches.frame_blend), (Quality::Wireframe, Sampling::Bicubic, FrameBlend::PixelMotion));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, a).switches.frame_blend, FrameBlend::Off);
    assert!(s.execute("layer.quality", json!({"quality": "fancy"})).is_err());
}

/// `layer.sampling {toggle}` (Alt+B, Alt+Shift+B): layers that all have the sampling already get
/// the other one; without `toggle` it is only set.
#[test]
fn layer_sampling_toggles_when_every_layer_has_it() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let sampling = |s: &Session| [layer(s, a).switches.sampling, layer(s, b).switches.sampling];
    let bicubic = json!({"layers": [a, b], "sampling": "bicubic", "toggle": true});
    s.execute("layer.sampling", bicubic.clone()).unwrap();
    assert_eq!(sampling(&s), [Sampling::Bicubic; 2]);
    s.execute("layer.sampling", bicubic.clone()).unwrap();
    assert_eq!(sampling(&s), [Sampling::Bilinear; 2]);
    s.execute("layer.sampling", json!({"layers": [a, b], "sampling": "bilinear", "toggle": true})).unwrap();
    assert_eq!(sampling(&s), [Sampling::Bicubic; 2]);
    // Mixed layers all get the one asked for.
    s.execute("layer.sampling", json!({"layers": [a], "sampling": "bilinear"})).unwrap();
    s.execute("layer.sampling", bicubic).unwrap();
    assert_eq!(sampling(&s), [Sampling::Bicubic; 2]);
    // Without `toggle` it stays.
    s.execute("layer.sampling", json!({"layers": [a, b], "sampling": "bicubic"})).unwrap();
    assert_eq!(sampling(&s), [Sampling::Bicubic; 2]);
}

/// `layer.setSwitch {others: true}` (Alt-click on an A/V feature): the layer stands alone, the
/// other layers come back the next time.
#[test]
fn set_switch_others() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let c = solid(&mut s, "#0000ff");
    let get = |s: &Session, f: fn(&effectcraft_project::Layer) -> bool| [a, b, c].map(|l| f(&layer(s, l)));
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "video", "value": false})).unwrap();
    // B is hidden: it is shown, alone; then the others come back; one undo step each.
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "video", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [false, true, false]);
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "video", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [true; 3]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [false, true, false]);
    // Solo.
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "solo", "value": true})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [c], "switch": "solo", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.solo), [false, false, true]);
    // Lock: the layer stays unlocked and the others lock, then unlock.
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.locked), [false, true, true]);
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.locked), [false; 3]);
    // A track matte keeps its video off.
    s.execute("layer.setTrackMatte", json!({"layer": b, "matte": a})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "video", "value": false})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [c], "switch": "video", "others": true})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [c], "switch": "video", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [false, true, true]);
    assert!(s.execute("layer.setSwitch", json!({"layers": [c], "switch": "nope", "others": true})).is_err());
}

/// The Video switch has a cell on every layer's row, cameras and lights included, so Alt-clicking
/// it (and Hide Other Video) takes them along with the rest.
#[test]
fn video_others_takes_cameras_and_lights() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    let light = s.execute("layer.newLight", json!({"kind": "Point"})).unwrap()["layer"].as_u64().unwrap();
    let get = |s: &Session, f: fn(&effectcraft_project::Layer) -> bool| [a, cam, light].map(|l| f(&layer(s, l)));
    // The camera and the light are hidden: Alt-click on the solid's eye brings them back, since
    // they stand apart with it, and the next Alt-click hides them with every other layer.
    s.execute("layer.setSwitch", json!({"layers": [cam, light], "switch": "video", "value": false})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "video", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [true; 3]);
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "video", "others": true})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [true, false, false]);
    // Hide Other Video takes them too.
    s.execute("layer.showAllVideo", json!({})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [true; 3]);
    s.execute("layer.hideOtherVideo", json!({"layers": [a]})).unwrap();
    assert_eq!(get(&s, |l| l.switches.video), [true, false, false]);
}

/// A null's Video switch (the eye) works like any layer's, as in After Effects: it toggles,
/// Alt-click and Hide Other Video take nulls along, and its children still follow it while it is
/// hidden (parenting ignores the eye).
#[test]
fn null_video_switch() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let n = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [n], "switch": "video"})).unwrap();
    assert!(!layer(&s, n).switches.video);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, n).switches.video);
    // Alt-click on the solid's eye hides the null too; on the null's, the solid.
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "video", "others": true})).unwrap();
    assert!(layer(&s, a).switches.video && !layer(&s, n).switches.video);
    s.execute("layer.setSwitch", json!({"layers": [n], "switch": "video", "others": true})).unwrap();
    assert!(!layer(&s, a).switches.video && layer(&s, n).switches.video);
    s.execute("layer.showAllVideo", json!({})).unwrap();
    s.execute("layer.hideOtherVideo", json!({"layers": [a]})).unwrap();
    assert!(layer(&s, a).switches.video && !layer(&s, n).switches.video);
    // The hidden null still carries its child.
    s.execute("layer.setParent", json!({"layers": [a], "parent": n})).unwrap();
    s.execute("prop.set", json!({"layer": n, "path": "transform/position", "value": [420, 180, 0]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let c = s.project.comp(cid).unwrap().clone();
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &c, Tick::ZERO);
    let child = c.layer(effectcraft_project::LayerId(a)).unwrap();
    let (m, _) = ctx.layer_to_comp(child);
    let centre = m.apply(effectcraft_geom::vec2(100.0, 50.0));
    assert!((centre.x - 420.0).abs() < 1e-6 && (centre.y - 180.0).abs() < 1e-6, "{centre:?}");
}

#[test]
fn layer_switches_menu() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("layer.hideOtherVideo", json!({})).unwrap();
    assert!(!layer(&s, a).switches.video && layer(&s, b).switches.video);
    s.execute("layer.showAllVideo", json!({})).unwrap();
    assert!(layer(&s, a).switches.video);
    s.execute("layer.setSwitch", json!({"layers": [a, b], "switch": "lock"})).unwrap();
    assert!(layer(&s, a).switches.locked);
    s.execute("layer.unlockAll", json!({})).unwrap();
    assert!(!layer(&s, a).switches.locked && !layer(&s, b).switches.locked);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, a).switches.locked);
    // Expressions on / off.
    s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "50"})).unwrap();
    s.execute("layer.expressions", json!({"layers": [a], "enabled": false})).unwrap();
    assert!(!layer(&s, a).props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().enabled);
    s.execute("layer.expressions", json!({"layers": [a], "enabled": true})).unwrap();
    assert!(layer(&s, a).props.prop("transform/opacity").unwrap().has_expression());
}

#[test]
fn transform_dialog_values_and_center_anchor() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("layer.setTransform", json!({"prop": "position", "value": [100, 50]})).unwrap();
    s.execute("layer.setTransform", json!({"prop": "rotation", "value": 90})).unwrap();
    s.execute("layer.setTransform", json!({"prop": "opacity", "value": 25})).unwrap();
    let l = layer(&s, a);
    assert_eq!(l.props.prop("transform/position").unwrap().value, KV::Vec3([100.0, 50.0, 0.0]));
    assert_eq!(l.props.prop("transform/opacity").unwrap().value.as_f64(), 25.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, a).props.prop("transform/opacity").unwrap().value.as_f64(), 100.0);
    assert!(s.execute("layer.setTransform", json!({"prop": "skew", "value": 1})).is_err());

    // Move the anchor off-centre, then Center Anchor Point keeps the layer in place.
    s.execute("layer.setTransform", json!({"prop": "anchor", "value": [0, 0]})).unwrap();
    s.execute("layer.setTransform", json!({"prop": "rotation", "value": 0})).unwrap();
    let before = layer(&s, a).props.prop("transform/position").unwrap().value.as_vec3();
    s.execute("layer.centerAnchor", json!({})).unwrap();
    let l = layer(&s, a);
    assert_eq!(l.props.prop("transform/anchor").unwrap().value.as_vec2(), [100.0, 50.0]);
    let after = l.props.prop("transform/position").unwrap().value.as_vec3();
    assert!((after[0] - before[0] - 100.0).abs() < 1e-6 && (after[1] - before[1] - 50.0).abs() < 1e-6, "{before:?} → {after:?}");

    s.execute("layer.autoOrient", json!({"mode": "alongPath"})).unwrap();
    assert_eq!(layer(&s, a).auto_orient, effectcraft_project::AutoOrient::AlongPath);
}

/// Inclusive bounds of samples whose alpha is above one half, plus the frame size and the count.
fn opaque_bounds(s: &Session, t: Tick) -> (u32, u32, i64, i64, i64, i64, u32) {
    let img = s.render(s.active_comp_id().unwrap(), t, Default::default());
    let mut n = 0u32;
    let mut x0 = i64::MAX;
    let mut y0 = i64::MAX;
    let mut x1 = i64::MIN;
    let mut y1 = i64::MIN;
    for y in 0..img.height as i64 {
        for x in 0..img.width as i64 {
            if img.get(x, y)[3] > 0.5 {
                n += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (img.width, img.height, x0, y0, x1, y1, n)
}

#[test]
fn separated_position_follows_canvas_resize_and_crop() {
    let square = |separated: bool| {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Test", "width": 100, "height": 100, "frameRate": 10, "duration": 2})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name": "Square", "color": "#ffffff", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setTransform", json!({"layer": a, "prop": "position", "value": [40, 50]})).unwrap();
        if separated {
            s.execute("prop.separateDimensions", json!({"layer": a, "value": true})).unwrap();
        }
        (s, a)
    };
    let placed = |s: &Session, a: u64, t: f64, x: f64, y: f64| {
        let l = layer(s, a);
        let tr = l.transform().unwrap();
        let at = Tick::from_seconds_f64(t);
        if tr.get("positionX").is_some() {
            assert!((tr.get("positionX").unwrap().value_at(at).as_f64() - x).abs() < 1e-6);
            assert!((tr.get("positionY").unwrap().value_at(at).as_f64() - y).abs() < 1e-6);
        }
        let p = tr.get("position").unwrap().value_at(at).as_vec3();
        assert!((p[0] - x).abs() < 1e-6 && (p[1] - y).abs() < 1e-6, "combined {p:?}");
    };

    let (mut combined, a) = square(false);
    let (mut separated, b) = square(true);
    assert_eq!(opaque_bounds(&combined, Tick::ZERO), opaque_bounds(&separated, Tick::ZERO));
    combined.execute("comp.settings", json!({"width": 200})).unwrap();
    separated.execute("comp.settings", json!({"width": 200})).unwrap();
    placed(&separated, b, 0.0, 90.0, 50.0);
    placed(&combined, a, 0.0, 90.0, 50.0);
    assert_eq!(opaque_bounds(&separated, Tick::ZERO), (200, 100, 85, 45, 94, 54, 100));
    assert_eq!(opaque_bounds(&combined, Tick::ZERO), opaque_bounds(&separated, Tick::ZERO));

    let (mut separated, b) = square(true);
    separated.execute("view.setRegionOfInterest", json!({"rect": [10, 20, 70, 70]})).unwrap();
    separated.execute("comp.cropToRegionOfInterest", json!({})).unwrap();
    placed(&separated, b, 0.0, 30.0, 30.0);
    assert_eq!(opaque_bounds(&separated, Tick::ZERO), (70, 70, 25, 25, 34, 34, 100));

    let (mut separated, b) = square(true);
    separated.execute("layer.select", json!({"layers": [b]})).unwrap();
    separated.execute("comp.cropToLayerBounds", json!({})).unwrap();
    let bounds = opaque_bounds(&separated, Tick::ZERO);
    assert_eq!((bounds.0, bounds.1, bounds.6), (10, 10, 100));
    assert!(bounds.2 == 0 && bounds.3 == 0, "the square fills the cropped frame: {bounds:?}");

    // Two Position keys keep their spacing after the same canvas offset.
    let (mut separated, b) = square(true);
    separated.execute("edit.undo", json!({})).unwrap();
    separated.execute("prop.addKey", json!({"layer": b, "path": "transform/position", "time": 0, "value": [40, 50]})).unwrap();
    separated.execute("prop.addKey", json!({"layer": b, "path": "transform/position", "time": 1, "value": [60, 50]})).unwrap();
    separated.execute("prop.separateDimensions", json!({"layer": b, "value": true})).unwrap();
    separated.execute("comp.settings", json!({"width": 200})).unwrap();
    placed(&separated, b, 0.0, 90.0, 50.0);
    placed(&separated, b, 1.0, 110.0, 50.0);
    assert_eq!(opaque_bounds(&separated, Tick::ZERO).2, 85);
    assert_eq!(opaque_bounds(&separated, Tick::from_seconds_f64(1.0)).2, 105);

    let (mut separated, b) = square(true);
    separated.execute("edit.undo", json!({})).unwrap();
    separated.execute("prop.addKey", json!({"layer": b, "path": "transform/position", "time": 0, "value": [40, 50]})).unwrap();
    separated.execute("prop.addKey", json!({"layer": b, "path": "transform/position", "time": 1, "value": [60, 50]})).unwrap();
    separated.execute("prop.separateDimensions", json!({"layer": b, "value": true})).unwrap();
    separated.execute("view.setRegionOfInterest", json!({"rect": [10, 20, 70, 70]})).unwrap();
    separated.execute("comp.cropToRegionOfInterest", json!({})).unwrap();
    placed(&separated, b, 0.0, 30.0, 30.0);
    placed(&separated, b, 1.0, 50.0, 30.0);
    assert_eq!(opaque_bounds(&separated, Tick::ZERO), (70, 70, 25, 25, 34, 34, 100));
    assert_eq!(opaque_bounds(&separated, Tick::from_seconds_f64(1.0)), (70, 70, 45, 25, 54, 34, 100));
}

/// Pixel-centre of the opaque samples, so a 10×10 solid centred on (11, 22) reports (11, 22).
fn opaque_centroid(s: &Session) -> (f64, f64, u32) {
    let img = s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default());
    let mut n = 0u32;
    let mut sx = 0.0;
    let mut sy = 0.0;
    for y in 0..img.height {
        for x in 0..img.width {
            if img.get(x as i64, y as i64)[3] > 0.5 {
                n += 1;
                sx += x as f64 + 0.5;
                sy += y as f64 + 0.5;
            }
        }
    }
    assert!(n > 0, "the layer rendered nothing");
    (sx / n as f64, sy / n as f64, n)
}

#[test]
fn separated_position_set_transform_moves_the_layer() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Scene", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "Square", "color": "#ff0000", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    let (x, y, n) = opaque_centroid(&s);
    assert_eq!(n, 100);
    assert!((x - 50.0).abs() < 0.6 && (y - 50.0).abs() < 0.6, "starts centred, centroid ({x}, {y})");

    s.execute("prop.separateDimensions", json!({"layer": a, "value": true})).unwrap();
    s.execute("layer.setTransform", json!({"layers": [a], "prop": "position", "value": [11, 22]})).unwrap();
    let l = layer(&s, a);
    let tr = l.transform().unwrap();
    assert!((tr.get("positionX").unwrap().value.as_f64() - 11.0).abs() < 1e-6);
    assert!((tr.get("positionY").unwrap().value.as_f64() - 22.0).abs() < 1e-6);
    assert_eq!(tr.get("position").unwrap().value, KV::Vec3([11.0, 22.0, 0.0]));
    let (x, y, n) = opaque_centroid(&s);
    assert_eq!(n, 100);
    assert!((x - 11.0).abs() < 0.6 && (y - 22.0).abs() < 0.6, "separated position renders at ({x}, {y})");

    s.execute("edit.undo", json!({})).unwrap();
    let (x, y, _) = opaque_centroid(&s);
    assert!((x - 50.0).abs() < 0.6 && (y - 50.0).abs() < 0.6, "undo puts it back at ({x}, {y})");

    // Combined Position, the control, still moves.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, a).transform().unwrap().get("positionX").is_none());
    s.execute("layer.setTransform", json!({"prop": "position", "value": [11, 22]})).unwrap();
    let (x, y, n) = opaque_centroid(&s);
    assert_eq!(n, 100);
    assert!((x - 11.0).abs() < 0.6 && (y - 22.0).abs() < 0.6, "combined position renders at ({x}, {y})");
}

#[test]
fn mask_menu_on_existing_masks() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    assert!(!s.is_enabled("layer.mask.reset"));
    s.execute("layer.addMask", json!({})).unwrap();
    s.execute("layer.addMask", json!({"shape": "ellipse"})).unwrap();
    assert!(s.is_enabled("layer.mask.reset"));
    let masks = |s: &Session| layer(s, a).masks().unwrap().groups().cloned().collect::<Vec<_>>();
    s.execute("layer.mask.set", json!({"field": "feather", "value": 12, "mask": 1})).unwrap();
    assert_eq!(masks(&s)[0].get("feather").unwrap().value, KV::Vec2([12.0, 12.0]));
    s.execute("layer.mask.set", json!({"field": "opacity", "value": 40})).unwrap();
    assert!(masks(&s).iter().all(|m| m.get("opacity").unwrap().value.as_f64() == 40.0));
    s.execute("layer.mask.invert", json!({"mask": 2})).unwrap();
    s.execute("layer.mask.mode", json!({"mode": "Subtract", "mask": 2})).unwrap();
    s.execute("layer.mask.lock", json!({"mask": 1})).unwrap();
    let kinds: Vec<_> = masks(&s).iter().map(|m| m.kind.clone()).collect();
    assert!(matches!(kinds[1], effectcraft_project::GroupKind::Mask { inverted: true, mode: effectcraft_project::MaskMode::Subtract, .. }));
    assert!(matches!(kinds[0], effectcraft_project::GroupKind::Mask { locked: true, .. }));
    s.execute("layer.mask.unlockAll", json!({})).unwrap();
    assert!(matches!(masks(&s)[0].kind, effectcraft_project::GroupKind::Mask { locked: false, .. }));
    s.execute("layer.mask.lockOthers", json!({"mask": 1})).unwrap();
    assert!(matches!(masks(&s)[1].kind, effectcraft_project::GroupKind::Mask { locked: true, .. }));
    s.execute("layer.mask.reset", json!({"mask": 1})).unwrap();
    assert_eq!(masks(&s)[0].get("opacity").unwrap().value.as_f64(), 100.0);
    s.execute("layer.mask.shape", json!({"mask": 1, "rect": [10, 10, 50, 40], "shape": "ellipse"})).unwrap();
    s.execute("layer.mask.remove", json!({"mask": 2})).unwrap();
    assert_eq!(masks(&s).len(), 1);
    s.execute("layer.mask.removeAll", json!({})).unwrap();
    assert!(masks(&s).is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(masks(&s).len(), 1);
}

#[test]
fn layer_markers_lock_and_delete() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("layer.addMarker", json!({"comment": "hit"})).unwrap();
    s.execute("layer.addMarker", json!({"time": 2.0})).unwrap();
    assert_eq!(layer(&s, a).markers.len(), 2);
    s.execute("layer.markersLock", json!({})).unwrap();
    assert!(layer(&s, a).markers_locked);
    s.execute("layer.deleteAllMarkers", json!({})).unwrap();
    assert_eq!(layer(&s, a).markers.len(), 2, "locked markers survive");
    s.execute("layer.markersLock", json!({"value": false})).unwrap();
    s.execute("layer.deleteAllMarkers", json!({})).unwrap();
    assert!(layer(&s, a).markers.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, a).markers.len(), 2);
}

#[test]
fn track_matte_menu() {
    let mut s = comp();
    let below = solid(&mut s, "#ff0000");
    let top = solid(&mut s, "#00ff00");
    s.execute("layer.select", json!({"layers": [below]})).unwrap();
    s.execute("layer.trackMatte", json!({"op": "luma"})).unwrap();
    let m = layer(&s, below).track_matte.unwrap();
    assert_eq!((m.layer.0, m.kind), (top, MatteKind::Luma));
    assert!(!layer(&s, top).switches.video);
    s.execute("layer.trackMatte", json!({"op": "alphaInverted"})).unwrap();
    assert_eq!(layer(&s, below).track_matte.unwrap().kind, MatteKind::AlphaInverted);
    s.execute("layer.trackMatte", json!({"op": "none"})).unwrap();
    assert!(layer(&s, below).track_matte.is_none());
    s.execute("layer.select", json!({"layers": [top]})).unwrap();
    assert!(s.execute("layer.trackMatte", json!({"op": "above"})).is_err());
    s.execute("layer.trackMatte", json!({"op": "below"})).unwrap();
    assert_eq!(layer(&s, top).track_matte.unwrap().layer.0, below);
}

#[test]
fn keyframe_clipboard_paste_and_paste_reversed() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/opacity", "value": 0})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(s.state.key_clipboard.iter().map(|c| c.keys.len()).sum::<usize>(), 2);
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    let keys = layer(&s, b).props.prop("transform/opacity").unwrap().keys.clone();
    assert_eq!(keys.len(), 2);
    assert!((keys[0].time.seconds() - 2.0).abs() < 1e-6 && keys[0].value.as_f64() == 100.0);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.pasteReversedKeyframes", json!({})).unwrap();
    let keys = layer(&s, b).props.prop("transform/opacity").unwrap().keys.clone();
    assert_eq!((keys[0].value.as_f64(), keys[1].value.as_f64()), (0.0, 100.0));
}

/// #493: Edit ▸ Copy with a property selected copies that property (its value when it isn't
/// animated, its keys when it is), and Paste sets the same property of the selected layer, as
/// in After Effects, instead of copying the whole layer.
#[test]
fn copy_a_selected_property_pastes_onto_the_same_property() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let n_layers = s.active_comp().unwrap().layers.len();
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [10, 20]})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/rotation", "value": 30})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "transform/position"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    assert!(s.state.clipboard.is_empty(), "the layer isn't copied");
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let undo = s.history.undo.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n_layers);
    assert_eq!(layer(&s, b).props.prop("transform/position").unwrap().value.as_vec3()[..2], [10.0, 20.0]);
    assert_eq!(layer(&s, b).props.prop("transform/rotation").unwrap().value.as_f64(), 0.0, "only the copied property");
    s.execute("edit.undo", json!({})).unwrap();
    assert_ne!(layer(&s, b).props.prop("transform/position").unwrap().value.as_vec3()[..2], [10.0, 20.0]);
    assert_eq!(s.history.undo.len(), undo);
    // The Transform group with an animated Opacity: its keys (at the CTI) and the other values,
    // in one undo step.
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/opacity", "value": 0})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 2);
    let transform = layer(&s, a).props.sub("transform").unwrap().uid;
    s.execute("prop.select", json!({"layer": a, "prop": transform, "selectKeys": false})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    let undo = s.history.undo.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let l = layer(&s, b);
    let keys = &l.props.prop("transform/opacity").unwrap().keys;
    assert_eq!(keys.len(), 2);
    assert!((keys[0].time.seconds() - 2.0).abs() < 1e-6 && keys[1].value.as_f64() == 0.0);
    assert_eq!(l.props.prop("transform/rotation").unwrap().value.as_f64(), 30.0);
    assert_eq!(s.active_comp().unwrap().layers.len(), n_layers);
    s.execute("edit.undo", json!({})).unwrap();
    let l = layer(&s, b);
    assert!(l.props.prop("transform/opacity").unwrap().keys.is_empty() && l.props.prop("transform/rotation").unwrap().value.as_f64() == 0.0);
}

#[test]
fn property_links_and_expression_only() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    s.execute("prop.select", json!({"layer": a, "path": "transform/position"})).unwrap();
    s.execute("edit.copyWithPropertyLinks", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    let e = layer(&s, b).props.prop("transform/position").unwrap().expr.clone().unwrap();
    let name = layer(&s, a).name;
    assert_eq!(e.text, format!("comp(\"Main\").layer({}).transform(\"Position\")", serde_json::to_string(&name).unwrap()));
    // Relative links use thisComp.
    s.execute("prop.select", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    s.execute("edit.copyWithRelativePropertyLinks", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert!(layer(&s, b).props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().text.starts_with("thisComp.layer("));
    // Copy Expression Only.
    s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": "time * 90"})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "transform/rotation"})).unwrap();
    s.execute("edit.copyExpressionOnly", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(layer(&s, b).props.prop("transform/rotation").unwrap().expr.as_ref().unwrap().text, "time * 90");
    // Layers with property links: the pasted copy follows the original.
    s.state.selected_props.clear();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("edit.copyWithPropertyLinks", json!({})).unwrap();
    let new = s.execute("edit.paste", json!({})).unwrap();
    let nid = new[0].as_u64().unwrap();
    assert!(layer(&s, nid).props.prop("transform/scale").unwrap().has_expression());
}

#[test]
fn lift_and_extract_work_area() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("comp.workArea", json!({"start": 2.0, "end": 4.0})).unwrap();
    s.execute("edit.liftWorkArea", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!(c.layers.len(), 2, "split around the gap");
    let mut spans: Vec<(f64, f64)> = c.layers.iter().map(|l| (l.in_point.seconds(), l.out_point.seconds())).collect();
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    assert!((spans[0].1 - 2.0).abs() < 1e-6 && (spans[1].0 - 4.0).abs() < 1e-6);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("edit.extractWorkArea", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    let mut spans: Vec<(f64, f64)> = c.layers.iter().map(|l| (l.in_point.seconds(), l.out_point.seconds())).collect();
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    assert!((spans[1].0 - 2.0).abs() < 1e-6 && (spans[1].1 - 8.0).abs() < 1e-6, "{spans:?}");
}

/// A project saved with an inverted work area (by an older build) is edited without a panic:
/// `clamp` with crossed bounds would abort.
#[test]
fn an_inverted_saved_work_area_takes_an_end_edit() {
    let mut s = comp();
    let cid = s.active_comp_id().unwrap();
    std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().work_area = (Tick::from_seconds_f64(9.0), Tick::from_seconds_f64(2.0));
    s.execute("comp.workArea", json!({"end": 1.0})).unwrap();
    s.execute("comp.workArea", json!({"start": 50.0})).unwrap();
    let c = s.active_comp().unwrap();
    assert!(c.work_area.0 >= Tick::ZERO && c.work_area.0 + c.frame_duration() <= c.work_area.1 && c.work_area.1 <= c.duration);
}

#[test]
fn work_area_stays_ordered_when_the_start_passes_the_end() {
    let ordered = |s: &Session| {
        let c = s.active_comp().unwrap();
        let fd = c.frame_duration();
        assert!(c.duration >= fd, "duration {}", c.duration.seconds());
        assert!(c.work_area.0 >= Tick::ZERO);
        assert!(c.work_area.0 + fd <= c.work_area.1, "start {} end {}", c.work_area.0.seconds(), c.work_area.1.seconds());
        assert!(c.work_area.1 <= c.duration);
        (c.duration, c.work_area)
    };
    let one_frame_at_two = |s: &Session| {
        let end = Tick::from_seconds_f64(2.0);
        (end - s.active_comp().unwrap().frame_duration(), end)
    };

    let mut s = comp();
    s.execute("comp.workArea", json!({"end": 2.0})).unwrap();
    s.execute("comp.workArea", json!({"start": 8.0})).unwrap();
    assert_eq!(ordered(&s).1, one_frame_at_two(&s));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ordered(&s).1, (Tick::ZERO, Tick::from_seconds_f64(2.0)));

    s.execute("comp.workArea", json!({"start": 0.0, "end": 2.0})).unwrap();
    s.execute("comp.trimToWorkArea", json!({})).unwrap();
    assert_eq!(ordered(&s), (Tick::from_seconds_f64(2.0), (Tick::ZERO, Tick::from_seconds_f64(2.0))));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ordered(&s).0, Tick::from_seconds_f64(10.0));

    let mut s = comp();
    s.execute("comp.workArea", json!({"start": 9.0, "end": 10.0})).unwrap();
    let before = ordered(&s);
    s.execute("comp.settings", json!({"duration": 2.0})).unwrap();
    let (dur, wa) = ordered(&s);
    assert_eq!(dur, Tick::from_seconds_f64(2.0));
    assert_eq!(wa, one_frame_at_two(&s));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ordered(&s), before);

    let mut s = comp();
    s.execute("comp.workArea", json!({"end": 2.0})).unwrap();
    s.execute("time.set", json!({"time": 8.0})).unwrap();
    s.execute("comp.workArea", json!({"set": "begin"})).unwrap();
    assert_eq!(ordered(&s).1, one_frame_at_two(&s));
    let path = tmp("WorkArea.ecproj");
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    s.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(ordered(&s).1, one_frame_at_two(&s));

    // A project that already has an inverted interval must not trim into a negative duration.
    let mut s = comp();
    let cid = s.state.active_comp.unwrap();
    {
        let c = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
        c.work_area = (Tick::from_seconds_f64(8.0), Tick::from_seconds_f64(2.0));
    }
    let err = s.execute("comp.trimToWorkArea", json!({})).unwrap_err();
    assert!(err.to_string().contains("work area"), "{err}");
    let c = s.active_comp().unwrap();
    assert_eq!(c.duration, Tick::from_seconds_f64(10.0));
    assert_eq!(c.work_area, (Tick::from_seconds_f64(8.0), Tick::from_seconds_f64(2.0)));
}

#[test]
fn label_group_purge_and_edit_original() {
    crate::tests_roto::hold_roto_cache_test_lock();
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let _c = solid(&mut s, "#0000ff");
    s.execute("edit.label", json!({"layers": [a, b], "label": "Cyan"})).unwrap();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("edit.selectLabelGroup", json!({})).unwrap();
    assert_eq!(s.state.selected_layers.len(), 2);
    // A footage source that counts purges (the media pool drops its decoded frames).
    struct Counting(std::sync::atomic::AtomicUsize);
    impl crate::render::FootageSource for Counting {
        fn frame(
            &self,
            _: effectcraft_project::ItemId,
            _: &effectcraft_project::Footage,
            _: effectcraft_time::Tick,
        ) -> Option<std::sync::Arc<crate::render::Image>> {
            None
        }
        fn purge(&self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let src = std::sync::Arc::new(Counting(Default::default()));
    s.footage = src.clone();
    s.drain_events();
    s.execute("edit.purge", json!({"what": "memory"})).unwrap();
    assert!(s.drain_events().contains(&Event::PurgeCaches));
    assert_eq!(src.0.load(std::sync::atomic::Ordering::SeqCst), 1, "decoded footage frames are dropped");
    // The disk cache and the snapshot purge alone: the RAM preview stays.
    for what in ["disk", "snapshot"] {
        s.execute("edit.purge", json!({"what": what})).unwrap();
        assert!(!s.drain_events().contains(&Event::PurgeCaches), "{what}");
    }
    assert_eq!(src.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    s.execute("edit.purge", json!({"what": "3d"})).unwrap();
    assert!(s.drain_events().contains(&Event::PurgeCaches));
    assert!(!s.is_enabled("edit.editOriginal"));
}

#[test]
fn animation_menu_keys_presets_text() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    // Add Keyframe on selected properties.
    s.execute("prop.select", json!({"layer": a, "path": "transform/scale"})).unwrap();
    s.execute("anim.addKeyframe", json!({})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/scale", "value": [400, 400]})).unwrap();
    assert_eq!(layer(&s, a).props.prop("transform/scale").unwrap().keys.len(), 2);
    // Exponential Scale: a key per frame, geometric in between.
    s.execute("prop.select", json!({"layer": a, "path": "transform/scale"})).unwrap();
    s.execute("keys.exponentialScale", json!({})).unwrap();
    let keys = layer(&s, a).props.prop("transform/scale").unwrap().keys.clone();
    assert_eq!(keys.len(), 31);
    assert!((keys[15].value.as_vec3()[0] - 200.0).abs() < 1e-6, "{:?}", keys[15].value);
    s.execute("edit.undo", json!({})).unwrap();
    // Time-Reverse Keyframes.
    s.execute("prop.select", json!({"layer": a, "path": "transform/scale"})).unwrap();
    s.execute("keys.timeReverse", json!({})).unwrap();
    let keys = layer(&s, a).props.prop("transform/scale").unwrap().keys.clone();
    assert_eq!((keys[0].value.as_vec3()[0], keys[1].value.as_vec3()[0]), (400.0, 100.0));

    // Presets round-trip through a file onto another layer at the CTI.
    let path = tmp("scale.ecpreset");
    s.execute("prop.select", json!({"layer": a, "path": "transform/scale"})).unwrap();
    s.execute("anim.savePreset", json!({"path": path})).unwrap();
    let b = solid(&mut s, "#00ff00");
    s.execute("time.set", json!({"time": 3.0})).unwrap();
    let r = s.execute("anim.applyPreset", json!({"path": path, "layers": [b]})).unwrap();
    assert_eq!(r["applied"], 1);
    let keys = layer(&s, b).props.prop("transform/scale").unwrap().keys.clone();
    assert_eq!(keys.len(), 2);
    assert!((keys[0].time.seconds() - 3.0).abs() < 1e-6);

    // Text: animate, add selector, reveal, remove all.
    let t = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addTextAnimator", json!({"properties": ["anchor", "position", "scale", "skew", "rotation", "opacity"]})).unwrap();
    s.execute("text.addSelector", json!({"kind": "range"})).unwrap();
    let anim = layer(&s, t).props.group("text/animators").unwrap().groups().next().unwrap().clone();
    assert_eq!(anim.sub("selectors").unwrap().children.len(), 2);
    // Skew brings Skew Axis along.
    assert_eq!(anim.sub("properties").unwrap().children.len(), 7);
    s.execute("text.addSelector", json!({"kind": "wiggly"})).unwrap();
    assert!(s.execute("text.addSelector", json!({"kind": "other"})).is_err());
    s.execute("text.removeAllAnimators", json!({})).unwrap();
    assert!(layer(&s, t).props.group("text/animators").unwrap().children.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, t).props.group("text/animators").unwrap().children.len(), 1);
}

#[test]
fn reveal_properties() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "transform/rotation"})).unwrap();
    s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "50"})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/scale", "value": [50, 50]})).unwrap();
    s.drain_events();
    let n = |v: &Value| v["props"].as_array().unwrap().len();
    assert_eq!(n(&s.execute("anim.reveal", json!({"kind": "keyframes"})).unwrap()), 1);
    assert_eq!(n(&s.execute("anim.reveal", json!({"kind": "animation"})).unwrap()), 2);
    assert_eq!(n(&s.execute("anim.reveal", json!({"kind": "modified"})).unwrap()), 3);
    assert!(frontend_events(&mut s).iter().all(|(c, _)| c == "timeline.revealProps"));
}

#[test]
fn reveal_layer_source_selects_each_layers_source_item() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let ia = layer(&s, a).source.item().unwrap();
    let ib = layer(&s, b).source.item().unwrap();
    assert_ne!(ia, ib);
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let r = s.execute("layer.revealSource", json!({})).unwrap();
    assert_eq!(r["item"], ia.0);
    assert_eq!(r["items"], json!([ia.0, ib.0]));
    assert_eq!(s.state.project_selection, vec![ia, ib]);
    assert_eq!(frontend_events(&mut s), vec![("project.reveal".to_string(), json!({"items": [ia.0, ib.0]}))]);
    // A null has no source. Alone it is refused; in a mixed selection only the sources are revealed.
    let n = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [n]})).unwrap();
    assert!(s.execute("layer.revealSource", json!({})).is_err());
    s.execute("layer.select", json!({"layers": [n, a]})).unwrap();
    let r = s.execute("layer.revealSource", json!({})).unwrap();
    assert_eq!(r["items"], json!([ia.0]));
    assert_eq!(s.state.project_selection, vec![ia]);
    // Reveal Composition in Project opens the same panel and folders.
    let cid = s.active_comp_id().unwrap();
    s.drain_events();
    let r = s.execute("comp.revealInProject", json!({"comp": cid.0})).unwrap();
    assert_eq!(r["item"], cid.0);
    assert_eq!(s.state.project_selection, vec![cid]);
    assert_eq!(frontend_events(&mut s), vec![("project.reveal".to_string(), json!({"items": [cid.0]}))]);
}

#[test]
fn sequence_layers() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    let c = solid(&mut s, "#0000ff");
    for (id, out) in [(a, 2.0), (b, 3.0), (c, 1.0)] {
        s.execute("layer.timing", json!({"layers": [id], "out": out})).unwrap();
    }
    s.execute("layer.sequence", json!({"layers": [a, b, c], "overlap": true, "duration": 0.5, "transition": "dissolveFront"})).unwrap();
    assert!((layer(&s, b).in_point.seconds() - 1.5).abs() < 1e-6);
    assert!((layer(&s, c).in_point.seconds() - 4.0).abs() < 1e-6);
    assert_eq!(layer(&s, c).props.prop("transform/opacity").unwrap().keys.len(), 2);
}

/// File ▸ Dependencies ▸ Find Missing Fonts lists a text layer only when no face matches its
/// font by family, PostScript or full name.
#[test]
fn fonts_named_by_their_postscript_or_full_name_are_not_missing() {
    let mut s = comp();
    // The first three name bundled faces by family, PostScript and full name; no face has the fourth.
    let fonts = ["Noto Serif", "Inter-Bold", "Inter SemiBold", "No Such Font Family 123"];
    let ids: Vec<u64> = fonts.iter().map(|f| s.execute("layer.newText", json!({"text": "Aa", "font": f})).unwrap()["layer"].as_u64().unwrap()).collect();
    let r = s.execute("file.findMissing", json!({"what": "fonts"})).unwrap();
    let listed: Vec<(u64, &str)> = r["fonts"].as_array().unwrap().iter().map(|f| (f["layer"].as_u64().unwrap(), f["font"].as_str().unwrap())).collect();
    assert_eq!(listed, [(ids[3], fonts[3])], "{r}");
    let f = s.execute("text.fontFeatures", json!({"layer": ids[1]})).unwrap();
    assert_eq!((&f["family"], &f["style"], &f["missing"]), (&json!("Inter"), &json!("Bold"), &json!(false)), "{f}");
}

#[test]
fn file_menu_project_ops() {
    let mut s = comp();
    let f = s.execute("project.newFolder", json!({"name": "Stuff"})).unwrap()["item"].as_u64().unwrap();
    assert!(s.project.item(effectcraft_project::ItemId(f)).unwrap().is_folder());
    let p1 = s.execute("file.importPlaceholder", json!({"name": "Shot 1", "width": 320, "height": 240})).unwrap()["item"].as_u64().unwrap();
    let p2 = s.execute("file.importPlaceholder", json!({"name": "Unused"})).unwrap()["item"].as_u64().unwrap();
    s.execute("file.importSolid", json!({"name": "Grey"})).unwrap();
    s.state.project_selection = vec![effectcraft_project::ItemId(p1)];
    let r = s.execute("file.newCompFromSelection", json!({})).unwrap();
    let nc = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    assert_eq!((s.project.comp(nc).unwrap().width, s.project.comp(nc).unwrap().layers.len()), (320, 1));
    // Missing footage = the placeholders.
    let miss = s.execute("file.findMissing", json!({"what": "footage"})).unwrap();
    assert_eq!(miss["items"].as_array().unwrap().len(), 2);
    assert!(s.execute("file.findMissing", json!({"what": "fonts"})).is_ok());
    // Interpretation.
    s.state.project_selection = vec![effectcraft_project::ItemId(p1)];
    s.execute("file.interpretFootage", json!({"frameRate": 24, "alpha": "premultiplied", "loop": 3})).unwrap();
    s.execute("file.rememberInterpretation", json!({})).unwrap();
    s.state.project_selection = vec![effectcraft_project::ItemId(p2)];
    s.execute("file.applyInterpretation", json!({})).unwrap();
    match &s.project.item(effectcraft_project::ItemId(p2)).unwrap().kind {
        ItemKind::Footage(f) => assert_eq!((f.alpha, f.loop_count, f.frame_rate.as_f64().round()), (effectcraft_project::AlphaMode::Premultiplied, 3, 24.0)),
        _ => panic!(),
    }
    // Remove Unused Footage drops the unused placeholder and the unused solid.
    let before = s.project.items.len();
    s.execute("file.removeUnusedFootage", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before - 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before);
    // Replace with a solid retargets the layer.
    s.state.project_selection = vec![effectcraft_project::ItemId(p1)];
    s.execute("file.replaceWithSolid", json!({"color": "#336699"})).unwrap();
    assert!(matches!(s.project.comp(nc).unwrap().layers[0].source, effectcraft_project::LayerSource::Solid { .. }));
    // Reduce Project keeps only the selected comp and what it uses (+ folders of kept items).
    s.state.project_selection = vec![nc];
    s.execute("file.reduceProject", json!({})).unwrap();
    assert!(s.project.items.len() <= 2, "{:?}", s.project.items.values().map(|i| &i.name).collect::<Vec<_>>());
    // Save a copy leaves the session path alone.
    let path = tmp("copy.ecproj");
    s.execute("file.saveCopy", json!({"path": path})).unwrap();
    assert!(s.path.is_none() && std::fs::metadata(&path).is_ok());
    s.execute("file.closeProject", json!({})).unwrap();
    assert!(s.project.items.is_empty());
}

#[test]
fn consolidate_duplicate_footage() {
    let mut s = comp();
    let f = |s: &mut Session| s.execute("file.importPlaceholder", json!({})).unwrap()["item"].as_u64().unwrap();
    let a = f(&mut s);
    let b = f(&mut s);
    // Give both the same file path.
    let mut p = (*s.project).clone();
    for id in [a, b] {
        if let Some(ItemKind::Footage(ft)) = p.item_mut(effectcraft_project::ItemId(id)).map(|i| &mut i.kind) {
            ft.path = "/media/shot.mov".into();
        }
    }
    s.project = std::sync::Arc::new(p);
    s.execute("layer.addItem", json!({"item": b})).unwrap();
    assert_eq!(s.execute("file.consolidateFootage", json!({})).unwrap()["removed"], 1);
    let c = s.active_comp().unwrap();
    assert_eq!(c.layers[0].source.item().unwrap().0, a);
}

#[test]
fn run_script_steps() {
    let mut s = comp();
    let path = tmp("script.jsonl");
    std::fs::write(&path, "// make two solids\n{\"command\":\"layer.newSolid\",\"params\":{\"color\":\"#ff0000\"}}\n{\"method\":\"engine.execute\",\"params\":{\"id\":\"layer.newNull\",\"params\":{}}}\n").unwrap();
    let r = s.execute("file.runScript", json!({"path": path})).unwrap();
    assert_eq!(r["steps"], 2);
    assert_eq!(s.active_comp().unwrap().layers.len(), 2);
    assert!(s.execute("file.runScript", json!({"steps": [{"command": "nope.nope"}]})).is_err());
}

#[test]
fn crop_comp_and_save_frame() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    // ROI crop moves layers so nothing shifts on screen.
    assert!(!s.is_enabled("comp.cropToRegionOfInterest"));
    s.execute("view.setRegionOfInterest", json!({"rect": [100, 50, 200, 100]})).unwrap();
    s.execute("comp.cropToRegionOfInterest", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!((c.width, c.height), (200, 100));
    assert_eq!(layer(&s, a).props.prop("transform/position").unwrap().value.as_vec2(), [220.0, 130.0]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().width, 640);
    // Crop to the selected layer's bounds (a 200×100 solid centred in 640×360).
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    let r = s.execute("comp.cropToLayerBounds", json!({})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(200), Some(100)));
    assert_eq!(layer(&s, a).props.prop("transform/position").unwrap().value.as_vec2(), [100.0, 50.0]);
    // Save Frame As ▸ File… writes a PNG.
    let path = tmp("frame.png");
    let r = s.execute("comp.saveFrameAs", json!({"path": path, "scale": 0.5})).unwrap();
    assert_eq!(r["width"], 100);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    // Responsive Design — Time.
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("comp.responsiveTime", json!({"op": "intro"})).unwrap();
    let m = &s.active_comp().unwrap().markers[0];
    assert!(m.protected && (m.duration.seconds() - 2.0).abs() < 1e-6);
}

#[test]
fn guides_add_clear_import_export() {
    let mut s = comp();
    s.execute("view.addGuide", json!({"orientation": "horizontal", "position": 40})).unwrap();
    s.execute("view.addGuide", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides.len(), 2);
    assert_eq!(s.active_comp().unwrap().guides[1].position, 320.0);
    let path = tmp("guides.json");
    s.execute("view.exportGuides", json!({"path": path})).unwrap();
    s.execute("view.clearGuides", json!({})).unwrap();
    assert!(s.active_comp().unwrap().guides.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides.len(), 2);
    s.execute("view.clearGuides", json!({})).unwrap();
    s.execute("view.importGuides", json!({"path": path})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides.len(), 2);
}

#[test]
fn frontend_commands_emit_events_and_stubs_are_disabled() {
    let mut s = comp();
    s.drain_events();
    s.execute("view.zoomIn", json!({})).unwrap();
    s.execute("window.panel", json!({"panel": "align"})).unwrap();
    let ev = frontend_events(&mut s);
    assert_eq!(ev[0].0, "view.zoomIn");
    assert_eq!(ev[1], ("window.panel".to_string(), json!({"panel": "align"})));
    for id in ["layer.autoTrace", "layer.sceneEditDetection", "layer.create", "track.motion"] {
        assert!(!s.is_enabled(id), "{id}");
        assert!(matches!(s.execute(id, json!({})), Err(EngineError::Disabled(..))), "{id}");
    }
}

fn solid_of(s: &Session, l: u64) -> (u64, effectcraft_project::Solid, String) {
    let effectcraft_project::LayerSource::Solid { item } = layer(s, l).source else { panic!("not a solid") };
    let it = s.project.item(item).unwrap();
    let ItemKind::Solid(so) = &it.kind else { panic!("not a solid item") };
    (item.0, so.clone(), it.name.clone())
}

#[test]
fn layer_settings_edit_the_solid_or_give_the_layer_its_own() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    let b = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    assert_eq!(solid_of(&s, a).0, solid_of(&s, b).0, "the duplicate shares the solid");
    // Nothing given: nothing recorded.
    let steps = s.history.undo.len();
    s.execute("layer.settings", json!({"layer": b})).unwrap();
    assert_eq!(s.history.undo.len(), steps);
    // Affect all off: the duplicate gets its own solid; the original keeps its colour.
    let r = s.execute("layer.settings", json!({"layer": b, "color": "#0000ff", "name": "Blue", "affectAll": false})).unwrap();
    let (bi, bs, bn) = solid_of(&s, b);
    assert_eq!(r, json!({"layer": b, "solid": bi}));
    assert_ne!(bi, solid_of(&s, a).0);
    assert_eq!((bs.color, bn.as_str(), layer(&s, b).name.as_str()), ([0.0, 0.0, 1.0], "Blue", "Blue"));
    assert_eq!(solid_of(&s, a).1.color, [1.0, 0.0, 0.0]);
    assert_eq!(
        s.project.item(effectcraft_project::ItemId(bi)).unwrap().parent,
        s.project.item(effectcraft_project::ItemId(solid_of(&s, a).0)).unwrap().parent,
        "same folder"
    );
    // Affect all (the default) changes the shared solid in place; size and pixel aspect too.
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    let c = s.execute("edit.duplicate", json!({})).unwrap()[0].as_u64().unwrap();
    s.execute("layer.settings", json!({"layer": a, "width": 320, "height": 90, "pixelAspect": 2.0})).unwrap();
    assert_eq!(solid_of(&s, a).0, solid_of(&s, c).0);
    let so = solid_of(&s, c).1;
    assert_eq!((so.width, so.height, so.pixel_aspect), (320, 90, 2.0));
    assert!(s.execute("layer.settings", json!({"layer": a, "pixelAspect": 0})).is_err());
    // One undo step each.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(solid_of(&s, a).1.width, 200);
    // A new solid takes a pixel aspect too.
    let d = s.execute("layer.newSolid", json!({"color": "#00ff00", "pixelAspect": 0.9091})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(solid_of(&s, d).1.pixel_aspect, 0.9091);
    // Text and shape layers have no settings.
    assert!(s.is_enabled("layer.settings"));
    s.execute("layer.newText", json!({"text": "T"})).unwrap();
    assert!(!s.is_enabled("layer.settings"));
}

fn order(s: &Session) -> Vec<u64> {
    s.active_comp().unwrap().layers.iter().map(|l| l.id.0).collect()
}

fn pos(s: &Session, l: u64) -> [f64; 3] {
    let ly = layer(s, l);
    let tr = ly.transform().unwrap();
    match tr.get("positionX") {
        Some(x) => [x.value.as_f64(), tr.get("positionY").unwrap().value.as_f64(), tr.get("positionZ").map(|z| z.value.as_f64()).unwrap_or(0.0)],
        None => tr.get("position").unwrap().value.as_vec3(),
    }
}

#[test]
fn locked_layers_are_not_selected_deleted_or_moved() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "lock", "value": true})).unwrap();
    assert!(layer(&s, b).switches.locked);
    s.execute("edit.selectAll", json!({})).unwrap();
    assert_eq!(s.state.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>(), vec![a]);
    // Ctrl+Up / Down step over it.
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("layer.selectPrevious", json!({})).unwrap();
    assert_eq!(s.state.selected_layers[0].0, a, "nothing above the locked layer to step to");
    // Deleting, arranging or transforming only the locked layer is refused; with others, it stays.
    let e = s.execute("edit.clear", json!({"layers": [b]})).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    assert!(s.execute("layer.arrange", json!({"layers": [b], "to": "back"})).is_err());
    assert!(s.execute("layer.transform", json!({"layers": [b], "op": "center"})).is_err());
    s.execute("edit.clear", json!({"layers": [a, b]})).unwrap();
    assert_eq!(order(&s), vec![b]);
}

#[test]
fn deleting_a_parent_keeps_its_children_in_place() {
    let mut s = comp();
    let parent = solid(&mut s, "#ff0000");
    s.execute("prop.set", json!({"layer": parent, "path": "transform/position", "value": [300, 200]})).unwrap();
    s.execute("prop.set", json!({"layer": parent, "path": "transform/rotation", "value": 90})).unwrap();
    let child = solid(&mut s, "#00ff00");
    s.execute("prop.set", json!({"layer": child, "path": "transform/position", "value": [210, 160]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [child], "parent": parent})).unwrap();
    assert_ne!(pos(&s, child)[..2], [210.0, 160.0], "expressed in the parent's space");
    s.execute("edit.clear", json!({"layers": [parent]})).unwrap();
    let p = pos(&s, child);
    assert!((p[0] - 210.0).abs() < 1e-6 && (p[1] - 160.0).abs() < 1e-6, "{p:?}");
    assert_eq!(layer(&s, child).parent, None);
    let r = layer(&s, child).transform().unwrap().get("rotation").unwrap().value.as_f64();
    assert!(r.abs() < 1e-6, "{r}");
    // One undo step restores both.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, child).parent.map(|l| l.0), Some(parent));
}

#[test]
fn bring_forward_and_send_backward_move_each_layer_one_step() {
    let mut s = comp();
    let ids: Vec<u64> = (0..4).map(|_| solid(&mut s, "#ffffff")).collect();
    let top = order(&s);
    assert_eq!(top, ids.iter().rev().copied().collect::<Vec<_>>());
    let (l0, l1, l2, l3) = (top[0], top[1], top[2], top[3]);
    // A non-contiguous selection: each moves down one step.
    s.execute("layer.arrange", json!({"layers": [l0, l2], "to": "backward"})).unwrap();
    assert_eq!(order(&s), vec![l1, l0, l3, l2]);
    // Forward again; a layer already at the top stays there.
    s.execute("layer.arrange", json!({"layers": [l0, l2], "to": "forward"})).unwrap();
    assert_eq!(order(&s), vec![l0, l1, l2, l3]);
    s.execute("layer.arrange", json!({"layers": [l0, l2], "to": "forward"})).unwrap();
    assert_eq!(order(&s), vec![l0, l2, l1, l3]);
    // Front / Back keep the layers' order.
    s.execute("layer.arrange", json!({"layers": [l0, l1], "to": "back"})).unwrap();
    assert_eq!(order(&s), vec![l2, l3, l0, l1]);
}

#[test]
fn transform_commands_keep_depth_and_follow_separated_dimensions() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [10, 20, -300]})).unwrap();
    s.execute("layer.transform", json!({"layers": [a], "op": "center"})).unwrap();
    assert_eq!(pos(&s, a), [320.0, 180.0, -300.0]);
    s.execute("layer.transform", json!({"layers": [a], "op": "fit"})).unwrap();
    assert_eq!(pos(&s, a), [320.0, 180.0, -300.0]);
    let b = solid(&mut s, "#00ff00");
    s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [10, 20]})).unwrap();
    s.execute("prop.separateDimensions", json!({"layer": b, "value": true})).unwrap();
    s.execute("layer.transform", json!({"layers": [b], "op": "center"})).unwrap();
    assert_eq!(pos(&s, b)[..2], [320.0, 180.0]);
    s.execute("layer.transform", json!({"layers": [b], "op": "reset"})).unwrap();
    assert_eq!(pos(&s, b)[..2], [320.0, 180.0]);
    // Center Anchor Point moves separated X / Y Position too, so the layer stays put.
    s.execute("prop.set", json!({"layer": b, "path": "transform/anchor", "value": [0, 0]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/positionX", "value": 100})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/positionY", "value": 50})).unwrap();
    s.execute("layer.centerAnchor", json!({"layers": [b]})).unwrap();
    assert_eq!(pos(&s, b)[..2], [200.0, 100.0], "the 200×100 solid's centre");
}

#[test]
fn duplicated_and_pasted_layers_keep_their_links() {
    let mut s = comp();
    let parent = solid(&mut s, "#ff0000");
    let matte = solid(&mut s, "#ffffff");
    let child = solid(&mut s, "#00ff00");
    s.execute("layer.setParent", json!({"layers": [child], "parent": parent})).unwrap();
    s.execute("layer.setTrackMatte", json!({"layer": child, "matte": matte, "kind": "alpha"})).unwrap();
    let m = layer(&s, child).track_matte.unwrap().layer;
    assert_eq!(m.0, matte);
    // Duplicating the child alone keeps the original parent and matte.
    let d = s.execute("edit.duplicate", json!({"layers": [child]})).unwrap()[0].as_u64().unwrap();
    assert_eq!((layer(&s, d).parent.map(|l| l.0), layer(&s, d).track_matte.map(|t| t.layer)), (Some(parent), Some(m)));
    // Duplicating parent and child together: the copy follows the copied parent.
    let r = s.execute("edit.duplicate", json!({"layers": [parent, child]})).unwrap();
    let (p2, c2) = (r[0].as_u64().unwrap(), r[1].as_u64().unwrap());
    assert_eq!(layer(&s, c2).parent.map(|l| l.0), Some(p2));
    // Copy / paste in the same composition keeps the links; in another one they are dropped.
    s.execute("layer.select", json!({"layers": [child]})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let pasted = s.execute("edit.paste", json!({})).unwrap()[0].as_u64().unwrap();
    assert_eq!(layer(&s, pasted).parent.map(|l| l.0), Some(parent));
    assert_eq!(layer(&s, pasted).track_matte.map(|t| t.layer), Some(m));
    s.execute("comp.new", json!({"name": "Other", "width": 64, "height": 64})).unwrap();
    let other = s.execute("edit.paste", json!({})).unwrap()[0].as_u64().unwrap();
    assert_eq!((layer(&s, other).parent, layer(&s, other).track_matte), (None, None));
}

#[test]
fn precompose_takes_the_compositions_settings() {
    let mut s = Session::default();
    s.execute(
        "comp.new",
        json!({"name": "Main", "width": 640, "height": 360, "pixelAspect": 2.0, "shutterAngle": 90, "motionBlurSamples": 8, "renderer": "advanced3D", "background": [0.2, 0.3, 0.4]}),
    )
    .unwrap();
    let a = solid(&mut s, "#ff0000");
    let r = s.execute("layer.precompose", json!({"layers": [a], "name": "Inner"})).unwrap();
    let inner = s.project.comp(effectcraft_project::ItemId(r["comp"].as_u64().unwrap())).unwrap();
    assert_eq!((inner.pixel_aspect, inner.shutter_angle, inner.motion_blur_samples), (2.0, 90.0, 8));
    assert_eq!((inner.renderer, inner.background), (effectcraft_project::Renderer::Advanced3D, [0.2, 0.3, 0.4]));
}

#[test]
fn new_comp_from_selection_is_one_undo_step_with_the_dialog_options() {
    let mut s = Session::default();
    let still = |s: &mut Session, name: &str, w: u32, par: f64| {
        let f = effectcraft_project::Footage {
            kind: effectcraft_project::FootageKind::Still,
            width: w,
            height: 100,
            pixel_aspect: par,
            has_video: true,
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut s.project).add_item(name, effectcraft_color::Label::None, None, ItemKind::Footage(f))
    };
    let a = still(&mut s, "a.png", 200, 1.0);
    let b = still(&mut s, "b.png", 300, 2.0);
    s.state.project_selection = vec![a, b];
    // Multiple compositions (the default): one per item, its size and pixel aspect; one undo step.
    let steps = s.history.undo.len();
    let r = s.execute("file.newCompFromSelection", json!({"duration": 4})).unwrap();
    let comps: Vec<u64> = r["comps"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(comps.len(), 2);
    let cb = s.project.comp(effectcraft_project::ItemId(comps[1])).unwrap();
    assert_eq!((cb.width, cb.pixel_aspect, cb.duration.seconds()), (300, 2.0, 4.0));
    assert_eq!(s.history.undo.len(), steps + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.comps().count(), 0);
    // One composition holding both, sized like the second, sequenced with a 1 s overlap, queued.
    s.state.project_selection = vec![a, b];
    let r = s
        .execute(
            "file.newCompFromSelection",
            json!({"single": true, "dimensionsFrom": 1, "duration": 3, "sequence": true, "overlap": true, "overlapDuration": 1, "addToRenderQueue": true}),
        )
        .unwrap();
    let c = s.project.comp(effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap())).unwrap();
    assert_eq!((c.width, c.pixel_aspect, c.duration.seconds()), (300, 2.0, 5.0));
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["a.png", "b.png"], "first selected on top");
    // Stills have no rate: the comp is 29.97 fps and the second layer starts on the frame nearest 2 s.
    assert_eq!(c.layers[0].in_point.seconds(), 0.0);
    assert!((c.layers[1].in_point.seconds() - 2.0).abs() < 1.0 / 29.97, "{}", c.layers[1].in_point.seconds());
    assert_eq!(s.project.render_queue.len(), 1);
    assert_eq!(s.history.undo.len(), steps + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.render_queue.is_empty() && s.project.comps().count() == 0);
    s.state.project_selection = vec![a, b];
    assert!(s.execute("file.newCompFromSelection", json!({"single": true, "dimensionsFrom": 2})).is_err());
}

#[test]
fn comps_made_from_the_same_footage_get_numbered_names_as_duplicate_does() {
    let mut s = Session::default();
    let add = |s: &mut Session, name: &str, kind: ItemKind| std::sync::Arc::make_mut(&mut s.project).add_item(name, effectcraft_color::Label::None, None, kind);
    let still = || {
        ItemKind::Footage(effectcraft_project::Footage {
            kind: effectcraft_project::FootageKind::Still,
            width: 200,
            height: 100,
            has_video: true,
            ..Default::default()
        })
    };
    let clip = add(&mut s, "clip.png", still());
    let other = add(&mut s, "other.png", still());
    let make = |s: &mut Session, sel: Vec<effectcraft_project::ItemId>, p: Value| -> Vec<String> {
        s.state.project_selection = sel;
        let r = s.execute("file.newCompFromSelection", p).unwrap();
        r["comps"].as_array().unwrap().iter().map(|c| s.project.item(effectcraft_project::ItemId(c.as_u64().unwrap())).unwrap().name.clone()).collect()
    };
    // The same footage again and again: "clip", "clip 2", "clip 3".
    assert_eq!(make(&mut s, vec![clip], json!({})), ["clip"]);
    assert_eq!(make(&mut s, vec![clip], json!({})), ["clip 2"]);
    assert_eq!(make(&mut s, vec![clip], json!({})), ["clip 3"]);
    // Several items at once (one comp each): every new comp has a name of its own.
    assert_eq!(make(&mut s, vec![clip, other, clip], json!({})), ["clip 4", "other", "clip 5"]);
    // One comp for all, named after the item it takes its settings from.
    assert_eq!(make(&mut s, vec![clip, other], json!({"single": true})), ["clip 6"]);
    assert_eq!(make(&mut s, vec![clip, other], json!({"single": true, "dimensionsFrom": 1})), ["other 2"]);
    // Undo removes the latest comp and frees its name for the next one.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.find_by_name("other 2").is_none());
    assert_eq!(make(&mut s, vec![other], json!({})), ["other 2"]);
    // A solid or extensionless footage still names its own first comp; the second is numbered.
    let red = add(&mut s, "Red Solid 1", ItemKind::Solid(effectcraft_project::Solid { color: [1.0, 0.0, 0.0], width: 100, height: 100, pixel_aspect: 1.0 }));
    assert_eq!(make(&mut s, vec![red], json!({})), ["Red Solid 1"]);
    assert_eq!(make(&mut s, vec![red], json!({})), ["Red Solid 2"]);
    // Duplicate numbers its copies the same way, from the same helper.
    let first = s.project.find_by_name("clip").unwrap().id;
    s.state.project_selection = vec![first];
    let r = s.execute("project.duplicate", json!({})).unwrap();
    assert_eq!(s.project.item(effectcraft_project::ItemId(r["items"][0].as_u64().unwrap())).unwrap().name, "clip 7");
    // A comp made from a comp doesn't take its source comp's name.
    assert_eq!(make(&mut s, vec![first], json!({})), ["clip 8"]);
    // Several comps at once are one undo step; undoing frees all their names.
    assert_eq!(make(&mut s, vec![clip, other], json!({})), ["clip 9", "other 3"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.find_by_name("clip 9").is_none() && s.project.find_by_name("other 3").is_none());
    assert_eq!(make(&mut s, vec![other, clip], json!({})), ["other 3", "clip 9"]);
}

#[test]
fn project_files_keep_their_format_warn_about_newer_versions_and_never_save_unreadable() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let path = tmp("Safe.ecproj");
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(effectcraft_project::saved_by(&saved).as_deref(), Some(effectcraft_project::APP_VERSION));
    // Save a Copy and Increment and Save keep an XML project XML.
    let copy = tmp("Copy.ecprojx");
    s.execute("file.saveCopy", json!({"path": copy})).unwrap();
    assert!(std::fs::read_to_string(&copy).unwrap().starts_with("<?xml"));
    s.execute("file.saveAs", json!({"path": tmp("Xml.ecprojx")})).unwrap();
    let r = s.execute("file.incrementAndSave", json!({})).unwrap();
    let inc = r["path"].as_str().unwrap().to_string();
    assert!(inc.ends_with("Xml 2.ecprojx"), "{inc}");
    assert!(std::fs::read_to_string(&inc).unwrap().starts_with("<?xml"));
    let r = s.execute("file.open", json!({"path": inc})).unwrap();
    assert_eq!(r["savedBy"], json!(effectcraft_project::APP_VERSION));
    // A project from a newer version opens with a warning.
    let newer = tmp("Newer.ecproj");
    std::fs::write(&newer, saved.replacen(effectcraft_project::APP_VERSION, "999.0.0", 1)).unwrap();
    s.drain_events();
    let r = s.execute("file.open", json!({"path": newer})).unwrap();
    assert_eq!(r["savedBy"], json!("999.0.0"));
    let warned = s.drain_events().into_iter().any(|e| matches!(e, Event::Toast { message, .. } if message.contains("newer than this version")));
    assert!(warned);
    // A value a project file can't hold: Save fails and the file on disk is left as it was.
    s.execute("file.open", json!({"path": path})).unwrap();
    let cid = s.state.active_comp.unwrap();
    std::sync::Arc::make_mut(&mut s.project)
        .comp_mut(cid)
        .unwrap()
        .layer_mut(effectcraft_project::LayerId(a))
        .unwrap()
        .props
        .prop_mut("transform/opacity")
        .unwrap()
        .value = KV::Scalar(f64::NAN);
    let e = s.execute("file.save", json!({})).unwrap_err().to_string();
    assert!(e.contains("can't be saved"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    // Not a project: a clear message.
    let junk = tmp("Junk.ecproj");
    std::fs::write(&junk, "{ not json").unwrap();
    let e = s.execute("file.open", json!({"path": junk})).unwrap_err().to_string();
    assert!(e.contains("is not a project EffectCraft can open"), "{e}");
}

#[test]
fn comp_settings_preserve_frame_rate_and_resolution() {
    let mut s = comp();
    let info = s.execute("comp.info", json!({})).unwrap();
    assert_eq!((info["preserveFrameRate"].clone(), info["preserveResolution"].clone()), (json!(false), json!(false)));
    s.execute_checked("comp.settings", json!({"preserveFrameRate": true, "preserveResolution": true})).unwrap();
    let info = s.execute("comp.info", json!({})).unwrap();
    assert_eq!((info["preserveFrameRate"].clone(), info["preserveResolution"].clone()), (json!(true), json!(true)));
    // Pre-compose keeps them; one undo step clears them.
    let a = solid(&mut s, "#ff0000");
    let r = s.execute("layer.precompose", json!({"layers": [a], "name": "Inner"})).unwrap();
    let inner = s.project.comp(effectcraft_project::ItemId(r["comp"].as_u64().unwrap())).unwrap();
    assert!(inner.preserve_frame_rate && inner.preserve_resolution);
    // Undo Pre-compose and the solid; then one more step undoes the settings.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().preserve_frame_rate);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.active_comp().unwrap().preserve_frame_rate && !s.active_comp().unwrap().preserve_resolution);
}

#[test]
fn new_comp_from_selection_survives_hostile_sequence_numbers() {
    let mut s = Session::default();
    let still = |s: &mut Session, name: &str| {
        let f = effectcraft_project::Footage {
            kind: effectcraft_project::FootageKind::Still,
            width: 64,
            height: 64,
            pixel_aspect: 1.0,
            has_video: true,
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut s.project).add_item(name, effectcraft_color::Label::None, None, ItemKind::Footage(f))
    };
    let a = still(&mut s, "a.png");
    let b = still(&mut s, "b.png");
    let c = still(&mut s, "c.png");
    for overlap in [json!(1e30), json!(-5), json!(f64::MAX)] {
        s.state.project_selection = vec![a, b, c];
        // A huge overlap overflowed the duration arithmetic (a panic with overflow checks, a
        // wrapped, too long comp without them).
        let r = s
            .execute("file.newCompFromSelection", json!({"single": true, "sequence": true, "overlap": true, "overlapDuration": overlap, "duration": 2}))
            .unwrap();
        let c = s.project.comp(effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap())).unwrap();
        assert!(c.duration.seconds() <= 6.0 && c.duration > effectcraft_time::Tick::ZERO, "{overlap}: {}", c.duration.seconds());
    }
    s.state.project_selection = vec![a, b];
    assert!(s.execute("file.newCompFromSelection", json!({"single": true, "dimensionsFrom": 99})).is_err());
}

fn key_times(s: &Session, l: u64) -> Vec<f64> {
    layer(s, l).props.prop("transform/opacity").unwrap().keys.iter().map(|k| (k.time.seconds() * 30.0).round() / 30.0).collect()
}

fn opacity_keys(s: &mut Session, l: u64, times: &[f64]) {
    for (i, t) in times.iter().enumerate() {
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": t, "value": i as f64 * 10.0})).unwrap();
    }
}

#[test]
fn dragging_a_key_past_another_keeps_it() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[1.0, 2.0]);
    s.execute("keys.select", json!({"keys": [{"layer": a, "path": "transform/opacity", "time": 1.0}]})).unwrap();
    // One drag, frame by frame, from 1 s over the 2 s key to 3 s.
    for _ in 0..60 {
        s.execute("keys.move", json!({"delta": 1.0 / 30.0, "merge": "key-drag"})).unwrap();
    }
    assert_eq!(key_times(&s, a), vec![2.0, 3.0], "the 2 s key passed over is still there");
    // Dropped exactly on a key: it replaces it, as in After Effects.
    s.history.merge_key = None;
    s.execute("keys.move", json!({"delta": -1.0, "merge": "key-drag-2"})).unwrap();
    assert_eq!(key_times(&s, a), vec![2.0]);
    // The whole drag was one undo step.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(key_times(&s, a), vec![1.0, 2.0]);
}

#[test]
fn keys_of_a_stretched_layer_move_with_the_pointer() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[0.0, 1.0]);
    s.execute_checked("layer.timeStretch", json!({"layers": [a], "percent": 200})).unwrap();
    assert_eq!(layer(&s, a).stretch, 200.0);
    let comp_time = |s: &Session, i: usize| {
        let l = layer(s, a);
        l.comp_time(l.props.prop("transform/opacity").unwrap().keys[i].time).seconds()
    };
    let before = comp_time(&s, 1);
    s.execute("keys.select", json!({"keys": [{"layer": a, "path": "transform/opacity", "time": 1.0}]})).unwrap();
    s.execute("keys.move", json!({"delta": 0.5})).unwrap();
    assert!((comp_time(&s, 1) - (before + 0.5)).abs() < 1.0 / 30.0, "{} → {}", before, comp_time(&s, 1));
    s.execute("keys.nudge", json!({"frames": 3})).unwrap();
    assert!((comp_time(&s, 1) - (before + 0.6)).abs() < 1.0 / 60.0, "three comp frames");
}

#[test]
fn key_selection_toggles_and_skips_locked_layers_and_hold_restores_bezier() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[0.0, 1.0]);
    let k = |t: f64| json!({"layer": a, "path": "transform/opacity", "time": t});
    s.execute("keys.select", json!({"keys": [k(0.0)]})).unwrap();
    s.execute("keys.select", json!({"keys": [k(1.0)], "toggle": true})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 2);
    s.execute("keys.select", json!({"keys": [k(0.0)], "toggle": true})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 1, "toggled out");
    // Hold off returns an eased key to Bezier.
    s.execute("keys.easyEase", json!({})).unwrap();
    s.execute("keys.toggleHold", json!({})).unwrap();
    s.execute("keys.toggleHold", json!({})).unwrap();
    let kf = layer(&s, a).props.prop("transform/opacity").unwrap().keys[1].clone();
    assert_eq!(kf.out_interp, effectcraft_keyframe::Interp::Bezier);
    // Locked: its keys can't be selected.
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "value": true})).unwrap();
    s.execute("keys.select", json!({"keys": [k(0.0)]})).unwrap();
    assert!(s.state.selected_keys.is_empty());
    s.execute("keys.selectAll", json!({"layers": [a]})).unwrap();
    assert!(s.state.selected_keys.is_empty());
}

#[test]
fn transform_drags_apply_their_whole_offset_to_the_keys_they_started_with() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[0.0, 1.0, 2.0]);
    s.execute("keys.selectAll", json!({"layers": [a]})).unwrap();
    // Squeezed onto one frame on the way, then back: the three keys survive.
    for k in [0.5, 0.01, 0.0001, 1.0] {
        s.execute("keys.transform", json!({"timeScale": k, "timeAnchor": 0.0, "merge": "box", "fromStart": true})).unwrap();
    }
    assert_eq!(key_times(&s, a), vec![0.0, 1.0, 2.0]);
    // A slow move in tiny steps still arrives (each step is the whole offset).
    s.history.merge_key = None;
    for i in 1..=30 {
        s.execute("keys.transform", json!({"timeOffset": i as f64 * 0.1 / 30.0, "merge": "move", "fromStart": true})).unwrap();
    }
    assert_eq!(key_times(&s, a), vec![0.1, 1.1, 2.1]);
}

#[test]
fn select_equal_previous_and_following_keyframes() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    for (t, v) in [(0.0, 20.0), (1.0, 50.0), (2.0, 20.0), (3.0, 70.0)] {
        s.execute("prop.addKey", json!({"layer": a, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    let pick = |s: &mut Session, t: f64| s.execute("keys.select", json!({"keys": [{"layer": a, "path": "transform/opacity", "time": t}]})).unwrap();
    let times = |s: &Session| {
        let mut v: Vec<f64> = s.state.selected_keys.iter().map(|k| k.time.seconds()).collect();
        v.sort_by(f64::total_cmp);
        v
    };
    pick(&mut s, 0.0);
    s.execute("keys.selectEqual", json!({})).unwrap();
    assert_eq!(times(&s), vec![0.0, 2.0]);
    pick(&mut s, 2.0);
    s.execute("keys.selectPrevious", json!({})).unwrap();
    assert_eq!(times(&s), vec![0.0, 1.0, 2.0]);
    pick(&mut s, 1.0);
    s.execute("keys.selectFollowing", json!({})).unwrap();
    assert_eq!(times(&s), vec![1.0, 2.0, 3.0]);
}

#[test]
fn keys_from_several_layers_paste_in_order_and_paste_reversed_respects_stretch() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    let b = solid(&mut s, "#00ff00");
    opacity_keys(&mut s, a, &[0.0, 1.0]);
    s.execute("prop.addKey", json!({"layer": b, "path": "transform/opacity", "time": 0.0, "value": 77})).unwrap();
    let c = solid(&mut s, "#0000ff");
    let d = solid(&mut s, "#ffffff");
    // Copy A's and B's Opacity keys, paste onto C and D: A → C, B → D (in order).
    s.execute("keys.selectAll", json!({"layers": [a, b]})).unwrap();
    s.execute("keys.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("layer.select", json!({"layers": [c, d]})).unwrap();
    s.execute("keys.paste", json!({})).unwrap();
    assert_eq!(key_times(&s, c), vec![2.0, 3.0], "A's two keys");
    assert_eq!(key_times(&s, d), vec![2.0], "B's one key");
    // Paste Reversed onto a layer stretched to 200 %: the reversed keys land 2 s apart there too.
    let e = solid(&mut s, "#808080");
    s.execute_checked("layer.timeStretch", json!({"layers": [e], "percent": 200})).unwrap();
    s.execute("keys.selectAll", json!({"layers": [a]})).unwrap();
    s.execute("keys.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 0.0})).unwrap();
    s.execute("layer.select", json!({"layers": [e]})).unwrap();
    s.execute("edit.pasteReversedKeyframes", json!({})).unwrap();
    let pr = layer(&s, e).props.prop("transform/opacity").unwrap().clone();
    let l = layer(&s, e);
    let ct: Vec<f64> = pr.keys.iter().map(|k| l.comp_time(k.time).seconds()).collect();
    let vals: Vec<f64> = pr.keys.iter().map(|k| k.value.as_f64()).collect();
    assert!(ct.len() == 2 && (ct[1] - ct[0] - 1.0).abs() < 1e-6, "keys 1 s apart in comp time: {ct:?}");
    assert_eq!(vals, vec![10.0, 0.0], "reversed");
}

/// #1425: a mask path (its value, or a keyframe of it) pasted onto Position becomes a motion
/// path, as in After Effects: a key per vertex with the path's tangents, over 2 s from the CTI,
/// the inner keys roving; a closed path returns to its start. AE pastes the layer-space numbers
/// as they are, so the source layer's and the target parent's transforms don't move them.
#[test]
fn a_copied_path_pastes_onto_position_as_a_motion_path() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    // A moved and scaled source: converting to comp space would give 2v + (200, 100).
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [400, 200]})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/scale", "value": [200, 200]})).unwrap();
    let rect = json!([[0, 0], [40, 0], [40, 40], [0, 40]]);
    s.execute("mask.new", json!({"layer": a, "vertices": rect.clone(), "outTangents": [[10, 0]], "closed": true})).unwrap();
    // B's parent would shift a converted Position by (0, −50).
    let parent = solid(&mut s, "#0000ff");
    s.execute("prop.set", json!({"layer": parent, "path": "transform/position", "value": [100, 100]})).unwrap();
    let b = solid(&mut s, "#00ff00");
    s.execute("layer.setParent", json!({"layers": [b], "parent": parent})).unwrap();
    let motion = |s: &Session, id: u64| {
        let l = layer(s, id);
        let pr = l.props.prop("transform/position").unwrap().clone();
        pr.keys.iter().map(|k| (l.comp_time(k.time).seconds(), k.value.as_vec3(), k.spatial_out, k.roving)).collect::<Vec<_>>()
    };
    let check = |keys: Vec<(f64, [f64; 3], [f64; 3], bool)>, t0: f64| {
        let want = [[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0], [0.0, 0.0]];
        assert_eq!(keys.len(), 5, "a key per vertex and the closing one: {keys:?}");
        for (i, ((t, v, _, roving), w)) in keys.iter().zip(want).enumerate() {
            assert!((v[0] - w[0]).abs() < 1e-6 && (v[1] - w[1]).abs() < 1e-6, "key {i}: {v:?}");
            assert_eq!(*roving, i != 0 && i != 4, "key {i} roving");
            assert!(*t > t0 - 1e-6 && *t < t0 + 2.0 + 1e-6, "key {i} at {t}");
        }
        assert!((keys[0].0 - t0).abs() < 1e-6 && (keys[4].0 - t0 - 2.0).abs() < 1e-6, "2 s from the CTI");
        assert!(keys.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(keys[0].2[..2], [10.0, 0.0], "the path's tangent");
    };
    // A Mask Path keyframe, pasted at 1 s.
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "masks/#1/path"})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "masks/#1/path"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.select", json!({"layer": b, "path": "transform/position"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    check(motion(&s, b), 1.0);
    // The same path as an unanimated mask, pasted at 5 s onto an unparented layer.
    s.execute("mask.new", json!({"layer": a, "vertices": rect, "outTangents": [[10, 0]], "closed": true})).unwrap();
    s.execute("prop.select", json!({"layer": a, "path": "masks/#2/path"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let c = solid(&mut s, "#ffffff");
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    s.execute("prop.select", json!({"layer": c, "path": "transform/position"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    check(motion(&s, c), 5.0);
}

#[test]
fn j_and_k_stop_at_keys_markers_and_the_work_area() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[2.0]);
    s.execute("comp.workArea", json!({"start": 1.0, "end": 3.0})).unwrap();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    let mut stops = vec![];
    for _ in 0..4 {
        s.execute("time.nextKey", json!({})).unwrap();
        stops.push((s.time().seconds() * 30.0).round() / 30.0);
    }
    let last = (3.0 * 30.0 - 1.0) / 30.0;
    assert_eq!(stops, vec![1.0, 2.0, (last * 30.0f64).round() / 30.0, (last * 30.0f64).round() / 30.0]);
}

#[test]
fn j_k_and_select_all_use_only_the_properties_the_timeline_shows() {
    let mut s = comp();
    let a = solid(&mut s, "#ff0000");
    opacity_keys(&mut s, a, &[1.0]);
    s.execute("prop.addKey", json!({"layer": a, "path": "transform/rotation", "time": 2.0, "value": 45})).unwrap();
    let op = layer(&s, a).props.prop("transform/opacity").unwrap().uid;
    // Only Opacity is revealed: K skips the hidden Rotation key at 2 s, stopping at 1 s and then
    // at the work area's end.
    let visible = json!([{"layer": a, "prop": op}]);
    s.execute_checked("time.nextKey", json!({"visible": visible})).unwrap();
    assert_eq!(s.time().seconds(), 1.0);
    s.execute_checked("time.nextKey", json!({"visible": visible})).unwrap();
    assert!(s.time().seconds() > 3.9, "work area end, not the hidden key: {}", s.time().seconds());
    // Without `visible` (agents), every key of the layers counts, as before.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("time.nextKey", json!({})).unwrap();
    assert_eq!(s.time().seconds(), 2.0);
    // Select All Keyframes: the shown properties' keys only.
    s.execute_checked("keys.selectAll", json!({"visible": visible})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 1);
    assert_eq!(s.state.selected_keys[0].prop, op);
}

/// A shape layer's Contents names, top to bottom.
fn contents_names(s: &Session, id: u64) -> Vec<String> {
    layer(s, id).props.sub("contents").unwrap().groups().map(|g| g.name.clone()).collect()
}

/// The uid of the shape item `name` at the top level of a shape layer's Contents.
fn contents_item(s: &Session, id: u64, name: &str) -> u64 {
    layer(s, id).props.sub("contents").unwrap().groups().find(|g| g.name == name).unwrap().uid
}

/// Edit ▸ Copy / Cut / Paste with shape items selected copy the items, not their layer: Paste
/// puts them into the selected shape layer, above its selected item or on top, and Cut + Paste
/// moves them (#227).
#[test]
fn shape_contents_copy_cut_and_paste_between_shape_layers() {
    let mut s = comp();
    let a = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("shape.newShape", json!({"layer": a, "kind": "ellipse"})).unwrap();
    let b = s.execute("layer.newShape", json!({"kind": "star", "name": "B"})).unwrap()["layer"].as_u64().unwrap();
    let n = s.active_comp().unwrap().layers.len();
    // Copy Rectangle 1 of A; paste into B (nothing of B's selected): on top of its Contents.
    let rect = contents_item(&s, a, "Rectangle 1");
    s.execute("prop.select", json!({"layer": a, "prop": rect})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap()["contents"], json!(1));
    assert!(s.state.clipboard.is_empty(), "not the layer");
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let r = s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(contents_names(&s, b), ["Rectangle 1", "Polystar 1"]);
    assert_eq!(s.active_comp().unwrap().layers.len(), n, "no new layer");
    let pasted = r["contents"][0].as_u64().unwrap();
    assert_ne!(pasted, rect);
    assert_eq!(s.state.selected_props, vec![(effectcraft_project::LayerId(b), pasted)]);
    // Again, with the pasted item selected: above it, with a unique name.
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(contents_names(&s, b), ["Rectangle 2", "Rectangle 1", "Polystar 1"]);
    s.undo();
    assert_eq!(contents_names(&s, b), ["Rectangle 1", "Polystar 1"]);
    assert_eq!(contents_names(&s, a), ["Ellipse 1", "Rectangle 1"], "the original stays");
    // Cut Ellipse 1 from A and paste it above B's Polystar 1: it moves.
    s.execute("prop.select", json!({"layer": a, "prop": contents_item(&s, a, "Ellipse 1")})).unwrap();
    s.execute("edit.cut", json!({})).unwrap();
    assert_eq!(contents_names(&s, a), ["Rectangle 1"]);
    assert_eq!(s.active_comp().unwrap().layers.len(), n, "the layer stays");
    s.execute("prop.select", json!({"layer": b, "prop": contents_item(&s, b, "Polystar 1")})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(contents_names(&s, b), ["Rectangle 1", "Ellipse 1", "Polystar 1"]);
    s.undo();
    s.undo();
    assert_eq!(contents_names(&s, a), ["Ellipse 1", "Rectangle 1"]);
    assert_eq!(contents_names(&s, b), ["Rectangle 1", "Polystar 1"]);
    // Only shape layers take shape items.
    let sol = solid(&mut s, "#ffffff");
    s.execute("layer.select", json!({"layers": [sol]})).unwrap();
    assert!(s.execute("edit.paste", json!({})).is_err());
}

/// Pasting Trim Paths into another shape layer with nothing selected puts it below the shapes,
/// where it acts on them (it acts on the items above it); on top it would trim nothing (#1681, #1629).
#[test]
fn pasted_path_operator_goes_below_the_shapes_it_acts_on() {
    let mut s = comp();
    let a = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let trim = s.execute("layer.addShapeItem", json!({"layer": a, "kind": "trim"})).unwrap()["uid"].as_u64().unwrap();
    let b = s.execute("layer.newShape", json!({"kind": "star", "name": "B"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.select", json!({"layer": a, "prop": trim})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(contents_names(&s, b), ["Polystar 1", "Trim Paths 1"]);
}

/// Pasting Merge Paths onto a painted layer with nothing selected keeps the paint: the renderer
/// drops the draws above a Merge Paths, so it goes between the path and the paints (#1681).
#[test]
fn pasted_merge_paths_keeps_the_targets_paint() {
    let mut s = comp();
    let a = s.execute("layer.newShape", json!({"kind": "none", "name": "A"})).unwrap()["layer"].as_u64().unwrap();
    let merge = s.execute("layer.addShapeItem", json!({"layer": a, "kind": "merge"})).unwrap()["uid"].as_u64().unwrap();
    let b = s.execute("layer.newShape", json!({"kind": "none", "name": "B"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addShapeItem", json!({"layer": b, "kind": "rect"})).unwrap();
    s.execute("layer.addShapeItem", json!({"layer": b, "kind": "fill"})).unwrap();
    let before = opaque_centroid(&s).2;
    s.execute("prop.select", json!({"layer": a, "prop": merge})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(contents_names(&s, b), ["Rectangle Path 1", "Merge Paths 1", "Fill 1"]);
    assert_eq!(opaque_centroid(&s).2, before);
}

/// Edit ▸ Duplicate with shape items selected duplicates them in their layer, above the
/// original and named "Rectangle 2", not the layer (#227).
#[test]
fn duplicate_with_shape_items_selected_duplicates_them_in_place() {
    let mut s = comp();
    let a = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let n = s.active_comp().unwrap().layers.len();
    let rect = contents_item(&s, a, "Rectangle 1");
    s.execute("prop.select", json!({"layer": a, "prop": rect})).unwrap();
    let r = s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(contents_names(&s, a), ["Rectangle 2", "Rectangle 1"]);
    assert_eq!(s.active_comp().unwrap().layers.len(), n, "no new layer");
    let copy = r["contents"][0].as_u64().unwrap();
    assert_eq!(s.state.selected_props, vec![(effectcraft_project::LayerId(a), copy)]);
    let l = layer(&s, a);
    let path_uid = |g: u64| l.props.find_group(g).unwrap().sub("contents").unwrap().groups().next().unwrap().uid;
    assert_ne!(path_uid(copy), path_uid(rect), "the copy has its own properties");
    // Again, with the copy selected: Rectangle 3, above it.
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(contents_names(&s, a), ["Rectangle 3", "Rectangle 2", "Rectangle 1"]);
    s.undo();
    s.undo();
    assert_eq!(contents_names(&s, a), ["Rectangle 1"]);
    // An item inside a group is duplicated in that group.
    let fill = layer(&s, a).props.find_group(rect).unwrap().sub("contents").unwrap().groups().find(|g| g.match_id == "fill").unwrap().uid;
    s.execute("prop.select", json!({"layer": a, "prop": fill})).unwrap();
    s.execute("edit.duplicate", json!({})).unwrap();
    let inner: Vec<String> = layer(&s, a).props.find_group(rect).unwrap().sub("contents").unwrap().groups().map(|g| g.name.clone()).collect();
    assert_eq!(inner, ["Rectangle Path 1", "Fill 2", "Fill 1"]);
    // With only the layer selected, the layer is duplicated as before.
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n + 1);
}

/// The Interpret Footage form sends "Loop (times)" as a float (3.0); it must still be applied (#909).
#[test]
fn interpret_footage_loop_from_form_float() {
    let mut s = comp();
    let p = s.execute("file.importPlaceholder", json!({"name": "Shot", "width": 320, "height": 240})).unwrap()["item"].as_u64().unwrap();
    s.state.project_selection = vec![effectcraft_project::ItemId(p)];
    s.execute("file.interpretFootage", json!({"loop": 3.0})).unwrap();
    match &s.project.item(effectcraft_project::ItemId(p)).unwrap().kind {
        ItemKind::Footage(f) => assert_eq!(f.loop_count, 3),
        _ => panic!(),
    }
}

/// Looped video footage lasts its loops: a 2 s clip looped 3 times is a 6 s layer, and a layer
/// that ran to the clip's end is re-timed when the loop count changes (#1045).
#[test]
fn interpret_footage_loop_extends_video_layers() {
    let mut s = comp();
    let p = s.execute("file.importPlaceholder", json!({"name": "Clip", "width": 320, "height": 240, "duration": 2})).unwrap()["item"].as_u64().unwrap();
    let before = s.execute("layer.addItem", json!({"item": p})).unwrap()["layer"].as_u64().unwrap();
    s.state.project_selection = vec![effectcraft_project::ItemId(p)];
    s.execute("file.interpretFootage", json!({"loop": 3.0})).unwrap();
    let after = s.execute("layer.addItem", json!({"item": p})).unwrap()["layer"].as_u64().unwrap();
    let out = |s: &Session, l: u64| s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap().out_point.seconds();
    assert_eq!(out(&s, after), 6.0, "a new layer plays every loop");
    assert_eq!(out(&s, before), 6.0, "a layer that ran to the clip's end still does");
    assert_eq!(s.project.item(effectcraft_project::ItemId(p)).unwrap().duration(), Some(Tick::from_seconds_f64(6.0)));
}

/// #1068: the same for an interpreted PNG sequence — the loop count survives interpretation and
/// a freshly dragged layer plays every cycle.
#[test]
fn interpret_footage_loop_extends_sequence_layers() {
    let mut s = comp();
    let p = s.execute("file.importPlaceholder", json!({"name": "Seq", "width": 320, "height": 240, "duration": 0.4})).unwrap()["item"].as_u64().unwrap();
    let id = effectcraft_project::ItemId(p);
    // Make it a sequence like the importer's grouped PNGs.
    s.edit("Seq", None, |proj, _| {
        if let Some(ItemKind::Footage(f)) = proj.item_mut(id).map(|i| &mut i.kind) {
            f.kind = effectcraft_project::FootageKind::Sequence;
            f.sequence = (0..12).map(|n| format!("/tmp/seq_{n:04}.png")).collect();
            f.sync_sequence_duration();
        }
        Ok(())
    })
    .unwrap();
    s.state.project_selection = vec![id];
    s.execute("file.interpretFootage", json!({"loop": 15.0})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": p})).unwrap()["layer"].as_u64().unwrap();
    let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap();
    let dur = (layer.out_point - layer.in_point).seconds();
    assert!((dur - 6.0).abs() < 0.05, "15 loops of a 0.4 s sequence: {dur}");
}

#[test]
fn percentage_guides_follow_the_comp_size() {
    let mut s = comp();
    s.execute("view.addGuide", json!({"orientation": "vertical", "position": 25, "percent": true})).unwrap();
    s.execute("view.addPercentGuide", json!({"orientation": "horizontal"})).unwrap();
    s.execute("view.addGuide", json!({"orientation": "vertical", "position": 100})).unwrap();
    let c = s.active_comp().unwrap().clone();
    assert!(c.guides[0].percent && c.guides[1].percent && !c.guides[2].percent);
    assert_eq!((c.guides[0].px(c.width, c.height), c.guides[1].px(c.width, c.height)), (160.0, 180.0));
    // A comp resize moves percentage guides with it and leaves pixel guides alone.
    s.execute("comp.settings", json!({"width": 1280, "height": 720})).unwrap();
    let c = s.active_comp().unwrap().clone();
    assert_eq!((c.guides[0].px(c.width, c.height), c.guides[1].px(c.width, c.height), c.guides[2].px(c.width, c.height)), (320.0, 360.0, 100.0));
    // Dragging keeps the unit: the position arrives in comp pixels.
    s.execute("view.moveGuide", json!({"index": 0, "position": 640})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides[0].position, 50.0);
    // Snapping sees the pixel position.
    let c = s.active_comp().unwrap().clone();
    assert!(c.guides.iter().any(|g| g.percent && g.vertical && g.px(c.width, c.height) == 640.0));
    // Converting keeps the guide where it is, and undoes in one step.
    s.execute("view.convertGuides", json!({"to": "pixels"})).unwrap();
    let g = s.active_comp().unwrap().guides.clone();
    assert!(g.iter().all(|g| !g.percent));
    assert_eq!((g[0].position, g[1].position), (640.0, 360.0));
    s.execute("view.convertGuides", json!({"to": "percent", "index": 2})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides[2].position, 100.0 / 1280.0 * 100.0);
    s.undo();
    assert!(!s.active_comp().unwrap().guides[2].percent);
    assert!(s.execute("view.convertGuides", json!({"to": "furlongs"})).is_err());
    assert!(s.execute("view.convertGuides", json!({"index": 9})).is_err());
    // A crop keeps percentage guides where they were on the image.
    s.execute("view.convertGuides", json!({"to": "percent"})).unwrap();
    s.execute("view.setRegionOfInterest", json!({"rect": [320, 0, 640, 720]})).unwrap();
    s.execute("comp.cropToRegionOfInterest", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert!((c.guides[0].px(c.width, c.height) - 320.0).abs() < 1e-9, "640 - 320 crop offset");
}

#[test]
fn old_guides_load_as_pixel_guides_and_percent_round_trips() {
    let old: effectcraft_project::Guide = serde_json::from_str(r#"{"vertical":true,"position":12.5}"#).unwrap();
    assert!(!old.percent);
    assert!(!serde_json::to_string(&old).unwrap().contains("percent"), "pixel guides stay in the old shape");
    let g = effectcraft_project::Guide { vertical: false, position: 10.0, percent: true };
    let back: effectcraft_project::Guide = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    assert_eq!(back, g);
    // A zero-sized comp never divides by zero.
    assert!(g.px(0, 0).is_finite());
}
