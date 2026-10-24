//! Headless checks for the 3D Camera Tracker UI (egui_kittest, UI logic only): the viewer's track
//! points and selection, the Effect Controls buttons and the Tracker panel's Track Camera button.
//! The solve is a synthetic one written into the effect (the analysis itself is tested in the
//! engine and track crates).

use effectcraft_engine::Session;
use effectcraft_engine::effects::camera_tracker as ct;
use effectcraft_engine::track::camtrack::{CameraSolve, ShotType, SolveMethod, SolvedFrame, SolvedPoint};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

const W: f64 = 640.0;
const H: f64 = 360.0;

fn app() -> (EffectcraftApp, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Shot", "width": W, "height": H, "duration": 2})).unwrap();
    let plate = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("effect.apply", json!({"effect": ct::ID, "layers": [plate]})).unwrap();
    let uid = r["effects"][0].as_u64().unwrap();
    // A synthetic solve: a static camera (focal 600) looking at a 6 × 4 grid of points on z = 1.
    let frames = 60;
    let solve = CameraSolve {
        version: 1,
        size: [W, H],
        start: 0.0,
        frame_duration: 1.0 / 30.0,
        shot: ShotType::FixedAngle,
        method_used: SolveMethod::Typical,
        average_error: 0.21,
        frames: vec![SolvedFrame { rot: [0.0; 3], center: [0.0; 3], focal: 600.0, solved: true }; frames],
        points: (0..24)
            .map(|i| SolvedPoint {
                id: i,
                pos: [((i % 6) as f64 - 2.5) * 0.15, ((i / 6) as f64 - 1.5) * 0.12, 1.0 + 0.01 * (i % 3) as f64],
                error: 0.2,
                first: 0,
                last: frames as u32 - 1,
            })
            .collect(),
        ground: None,
        distortion: None,
    };
    s.execute("prop.set", json!({"layer": plate, "path": "effects/#1/solve", "value": solve.to_json()})).unwrap();
    s.camera_pending.clear();
    s.state.selected_props = vec![(effectcraft_engine::project::LayerId(plate), uid)];
    s.execute("layer.select", json!({"layers": [plate]})).unwrap();
    s.state.selected_props = vec![(effectcraft_engine::project::LayerId(plate), uid)];
    (EffectcraftApp::new(s), plate, uid)
}

fn ids(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect()
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, at: egui::Pos2, modifiers: egui::Modifiers) {
    h.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(3);
    h.input_mut().events.push(egui::Event::ModifiersChanged(Default::default()));
    h.step();
}

#[test]
fn deleting_viewer_track_points_preserves_the_effect_and_layer() {
    let (a, plate, uid) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| a);
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(4);
    let point = rect(&h, "viewer.cameraTracker.point.7").center();
    click(&mut h, point, Default::default());
    h.state_mut().ui.focused = PanelKind::Composition;
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(3);
    let layer = h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(plate)).unwrap();
    let effect = layer.props.find_group(uid).expect("tracker effect survived");
    let params = effectcraft_engine::camera_track::static_params(effect);
    assert!(ct::deleted(&params).contains(&7));
}

#[test]
fn viewer_points_selection_and_effect_controls() {
    let (a, plate, uid) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| a);
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(4);
    let all = ids(&h);
    assert!(all.iter().any(|i| i == "viewer.cameraTracker"), "track point overlay registered");
    let mut pts: Vec<&String> = all.iter().filter(|i| i.starts_with("viewer.cameraTracker.point.")).collect();
    pts.sort();
    pts.dedup();
    let n = pts.len();
    assert_eq!(n, 24, "every visible point is drawn");
    // Click a point: it is selected; Shift-click another adds it.
    let r = rect(&h, "viewer.cameraTracker.point.7");
    click(&mut h, r.center(), Default::default());
    assert_eq!(h.state().session.state.camera_points, vec![7]);
    let r = rect(&h, "viewer.cameraTracker.point.9");
    click(&mut h, r.center(), egui::Modifiers::SHIFT);
    assert_eq!(h.state().session.state.camera_points, vec![7, 9]);
    // Marquee over the first row selects its six points.
    let (a0, a5) = (rect(&h, "viewer.cameraTracker.point.0"), rect(&h, "viewer.cameraTracker.point.5"));
    let from = a0.left_top() - egui::vec2(6.0, 6.0);
    let to = a5.right_bottom() + egui::vec2(6.0, 6.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    let mut sel = h.state().session.state.camera_points.clone();
    sel.sort();
    assert_eq!(sel, vec![0, 1, 2, 3, 4, 5]);
    // The right-click menu's Create Solid and Camera (via its command).
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "camera.createFromSolve", json!({"kind": "solid"})).unwrap();
    let comp = h.state().session.active_comp().unwrap();
    assert!(comp.layers.iter().any(|l| l.name == "3D Tracker Camera" && l.is_camera()));
    assert!(comp.layers.iter().any(|l| l.name.starts_with("Track Solid")));

    // Effect Controls: Analyze and Create Camera.
    h.state_mut().session.execute("layer.select", json!({"layers": [plate]})).unwrap();
    h.state_mut().show_panel(PanelKind::EffectControls);
    h.run_steps(4);
    let all = ids(&h);
    for want in [format!("effectControls.cameraTracker.{uid}.analyze"), format!("effectControls.cameraTracker.{uid}.createCamera")] {
        assert!(all.contains(&want), "missing {want}");
    }
    // Tracker panel: Track Camera is available for the selected layer.
    h.state_mut().show_panel(PanelKind::Tracker);
    h.run_steps(3);
    assert!(ids(&h).iter().any(|i| i == "tracker.trackCamera"));
    assert!(h.state().session.is_enabled("track.camera"));
}

/// Drag with `button` from `from` to `to` in steps, then release.
fn drag_with(h: &mut Harness<'_, EffectcraftApp>, button: egui::PointerButton, from: egui::Pos2, to: egui::Pos2) {
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton { pos: to, button, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

/// #505: with the track points shown, the wheel still zooms the viewer, and a middle-button drag
/// and a Spacebar (Hand tool) drag over the points still pan it.
#[test]
fn viewer_zooms_and_pans_over_the_track_points() {
    let (a, ..) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| a);
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(4);
    // Wheel over a point: zooms.
    let at = rect(&h, "viewer.cameraTracker.point.7").center();
    let zoom = h.state().ui.viewer.zoom;
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 120.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(20);
    assert_ne!(h.state().ui.viewer.zoom, zoom, "the wheel zooms over the track points");
    // Middle-button drag from a point: pans, and selects nothing.
    let at = rect(&h, "viewer.cameraTracker.point.7").center();
    let before = h.state().ui.viewer.pan;
    drag_with(&mut h, egui::PointerButton::Middle, at, at + egui::vec2(120.0, 60.0));
    let after = h.state().ui.viewer.pan;
    assert!((after[0] - before[0] - 120.0).abs() < 1.0 && (after[1] - before[1] - 60.0).abs() < 1.0, "{before:?} → {after:?}");
    assert!(h.state().session.state.camera_points.is_empty(), "a pan selects no points");
    // Spacebar held: a primary drag from a point pans instead of drawing a marquee.
    let at = rect(&h, "viewer.cameraTracker.point.7").center();
    let space = |h: &mut Harness<'_, EffectcraftApp>, pressed: bool| {
        h.input_mut().events.push(egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Default::default() });
        h.step();
    };
    space(&mut h, true);
    drag_with(&mut h, egui::PointerButton::Primary, at, at + egui::vec2(80.0, 40.0));
    space(&mut h, false);
    let end = h.state().ui.viewer.pan;
    assert!((end[0] - after[0] - 80.0).abs() < 1.0 && (end[1] - after[1] - 40.0).abs() < 1.0, "{after:?} → {end:?}");
    assert!(h.state().session.state.camera_points.is_empty(), "a Spacebar pan selects no points");
}

/// Step the UI until background frame renders have landed.
fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for _ in 0..4 {
        h.step();
    }
}

#[test]
#[ignore = "analyses the demo's 3D Showcase and renders with wgpu; run with --ignored (set EC_SNAPSHOT_DIR to keep PNGs)"]
fn snapshot_showcase_track() {
    let dir = std::env::var("EC_SNAPSHOT_DIR").ok();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("comp.new", json!({"name": "Camera Track", "width": 1920, "height": 1080, "duration": 4, "frameRate": 29.97})).unwrap();
    s.execute("layer.addItem", json!({"item": "3D Showcase"})).unwrap();
    s.execute("layer.select", json!({"layers": ["#1"]})).unwrap();
    s.execute("track.camera", json!({"layer": "#1", "wait": true})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Composition);
    settle(&mut h);
    let pts: Vec<String> = ids(&h).into_iter().filter(|i| i.starts_with("viewer.cameraTracker.point.")).collect();
    assert!(pts.len() > 10, "{}", pts.len());
    // Hover between points to show a target, then select three and show the selection's target.
    let r = rect(&h, &pts[0]);
    h.input_mut().events.push(egui::Event::PointerMoved(r.center() + egui::vec2(12.0, 12.0)));
    h.run_steps(2);
    if let Some(d) = &dir {
        h.render().expect("render").save(format!("{d}/camera_tracker_hover.png")).unwrap();
    }
    let sel: Vec<u64> = pts.iter().take(5).filter_map(|i| i.rsplit('.').next()?.parse().ok()).collect();
    h.state_mut().session.execute("camera.selectPoints", json!({"points": sel})).unwrap();
    h.state_mut().session.execute("camera.createFromSolve", json!({"kind": "solid"})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": ["#3"]})).unwrap();
    settle(&mut h);
    if let Some(d) = &dir {
        h.render().expect("render").save(format!("{d}/camera_tracker_solid.png")).unwrap();
    }
}
