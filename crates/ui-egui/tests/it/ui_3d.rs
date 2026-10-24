//! Headless UI checks for the 3D viewer (egui_kittest). The `snapshot` test renders the window
//! with wgpu and writes PNGs when `EC_SNAPSHOT_DIR` is set (`cargo test -p effectcraft-ui-egui
//! --test it ui_3d:: -- --ignored`); the other test only runs the UI logic.

use effectcraft_engine::Session;
use effectcraft_engine::render::three_d::View3D;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("comp.open", json!({"comp": "3D Showcase"})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    EffectcraftApp::new(s)
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
fn viewer_registers_3d_controls() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    settle(&mut h);
    h.step();
    let ids: Vec<String> = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect();
    for want in ["viewer.view3d", "viewer.renderer3d"] {
        assert!(ids.iter().any(|i| i == want), "missing {want}");
    }
    assert!(ids.iter().any(|i| i.starts_with("viewer.light.")), "light wireframe registered");
    // In a custom view the active camera is drawn as a wireframe too.
    h.state_mut().session.execute("view.3d.custom1", json!({})).unwrap();
    settle(&mut h);
    h.step();
    let ids: Vec<String> = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect();
    assert!(ids.iter().any(|i| i.starts_with("viewer.camera.")), "camera wireframe registered");
}

/// Click the centre of the automation element `id`.
fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    let p = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

/// #307: the viewer's 3D Renderer menu shows the comp's renderer (it always read "Classic 3D")
/// and switches it, as After Effects' Composition panel does.
#[test]
fn renderer_menu_shows_and_switches_the_comp_renderer() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.state_mut().session.execute("comp.settings", json!({"renderer": "classic3D"})).unwrap();
    h.run_steps(3);
    let label = |h: &Harness<'_, EffectcraftApp>| h.state().auto.find("viewer.renderer3d").map(|e| e.label.clone());
    assert_eq!(label(&h).as_deref(), Some("Classic 3D"));
    click_id(&mut h, "viewer.renderer3d");
    click_id(&mut h, "viewer.renderer3dItem.1");
    let renderer = h.state().session.active_comp().unwrap().renderer;
    assert_eq!(renderer, effectcraft_engine::project::Renderer::Advanced3D);
    h.run_steps(2);
    assert_eq!(label(&h).as_deref(), Some("Advanced 3D"));
    // One undoable Composition Settings step.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.active_comp().unwrap().renderer, effectcraft_engine::project::Renderer::Classic3D);
}

#[test]
fn camera_and_light_dialogs_create_layers() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.step();
    let ctx = h.ctx.clone();
    let n0 = h.state().session.active_comp().unwrap().layers.len();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.newCamera", json!({})).unwrap();
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::CameraSettings));
    h.step();
    let ok = h.state().auto.elements.iter().chain(h.state().auto.previous.iter()).find(|e| e.id == "dialog.camera.ok").map(|e| e.rect);
    assert!(ok.is_some(), "camera dialog OK registered");
    // OK through the dialog state (Enter).
    h.key_press(egui::Key::Enter);
    h.step();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), n0 + 1);
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.newLight", json!({})).unwrap();
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LightSettings));
    h.step();
    h.key_press(egui::Key::Enter);
    h.step();
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), n0 + 2);
    // Layer Settings on the new light reopens Light Settings for it.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.settings", json!({})).unwrap();
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::LightSettings));
}

#[test]
#[ignore = "renders with wgpu; run with --ignored (set EC_SNAPSHOT_DIR to keep PNGs)"]
fn snapshot() {
    let dir = std::env::var("EC_SNAPSHOT_DIR").ok();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|_| app());
    let shots: [(&str, Option<&str>); 4] =
        [("active", None), ("custom1", Some("view.3d.custom1")), ("top", Some("view.3d.top")), ("left", Some("view.3d.left"))];
    for (name, cmd) in shots {
        if let Some(c) = cmd {
            h.state_mut().session.execute(c, json!({})).unwrap();
        }
        settle(&mut h);
        let img = h.render().expect("render");
        if let Some(d) = &dir {
            img.save(format!("{d}/ui3d_{name}.png")).unwrap();
        }
    }
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.newCamera", json!({})).unwrap();
    h.run_steps(3);
    let img = h.render().expect("render");
    if let Some(d) = &dir {
        img.save(format!("{d}/ui3d_camera_dialog.png")).unwrap();
    }
    h.state_mut().dialog = None;
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.newLight", json!({})).unwrap();
    h.run_steps(3);
    let img = h.render().expect("render");
    if let Some(d) = &dir {
        img.save(format!("{d}/ui3d_light_dialog.png")).unwrap();
    }
    // A 3D layer's rotation properties (R) and the selected layer's gizmo in the active camera.
    h.state_mut().dialog = None;
    h.state_mut().session.execute("view.3d.activeCamera", json!({})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": ["Card Blue"]})).unwrap();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.rotation", json!({})).unwrap();
    settle(&mut h);
    let img = h.render().expect("render");
    if let Some(d) = &dir {
        img.save(format!("{d}/ui3d_reveal_rotation.png")).unwrap();
    }
}

/// Settings ▸ 3D ▸ Extended Viewer: a custom 3D view renders the pasteboard around the comp
/// frame, and 3D layers reaching past the frame draw pixels there.
#[test]
fn extended_viewer_renders_past_the_comp_frame() {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.state_mut().session.execute("view.3d.custom1", json!({})).unwrap();
    // Zoom out so the viewer shows plenty of pasteboard.
    h.state_mut().ui.viewer.zoom = Some(0.15);
    settle(&mut h);
    assert!(h.state().ui.viewer.extended.is_none(), "off by default");
    let ids: Vec<String> = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect();
    assert!(ids.iter().any(|i| i == "viewer.extendedViewer"), "toggle registered");
    assert_eq!(h.state_mut().session.execute("view.extendedViewer", json!({"value": true})).unwrap(), json!(true));
    h.step();
    settle(&mut h);
    h.step();
    settle(&mut h);
    let region = h.state().ui.viewer.extended.expect("extended region in a custom view");
    let (cw, ch) = {
        let c = h.state().session.active_comp().unwrap();
        (c.width as f64, c.height as f64)
    };
    assert!(region[0] < 0.0 && region[1] < 0.0 && region[0] + region[2] > cw && region[1] + region[3] > ch, "{region:?}");
    let ids: Vec<String> = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect();
    assert!(ids.iter().any(|i| i == "viewer.extendedArea"));
    let img = h.state_mut().viewer_pixels().expect("frame");
    let s = img.size[0] as f64 / region[2];
    assert!((img.size[1] as f64 / region[3] - s).abs() < 0.05, "frame covers the region");
    // Opaque pixels outside the comp rectangle.
    let (fx0, fy0) = (-region[0] * s, -region[1] * s);
    let (fx1, fy1) = (fx0 + cw * s, fy0 + ch * s);
    let mut outside = 0;
    for y in 0..img.size[1] {
        for x in 0..img.size[0] {
            let (xf, yf) = (x as f64 + 0.5, y as f64 + 0.5);
            if (xf < fx0 - 1.0 || xf > fx1 + 1.0 || yf < fy0 - 1.0 || yf > fy1 + 1.0) && img.pixels[y * img.size[0] + x].a() > 0 {
                outside += 1;
            }
        }
    }
    assert!(outside > 50, "pixels drawn on the pasteboard: {outside}");
    // The Active Camera view (Draft 3D off) shows just the frame.
    h.state_mut().session.execute("view.3d.activeCamera", json!({})).unwrap();
    h.step();
    h.step();
    assert!(h.state().ui.viewer.extended.is_none());
}

/// #875: the Top / Front / Right views of a 4-view layout draw the camera wireframe (they drew
/// none, so moving the camera showed nothing there), and it follows the camera's Position.
#[test]
fn layout_views_draw_the_camera_and_follow_its_position() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    let cam = s.execute("layer.newCamera", json!({"position": [320, 180, -300], "poi": [320, 180, 0]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("view.layout", json!({"views": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    let centre = |h: &Harness<'_, EffectcraftApp>, view: &str| {
        let id = format!("viewer.view.{view}.camera.{cam}");
        let [x, y, w, hh] = h.state().auto.find(&id).unwrap_or_else(|| panic!("{id} is registered")).rect;
        (x + w / 2.0, y + hh / 2.0)
    };
    for v in ["top", "front", "right"] {
        centre(&h, v);
    }
    let before = centre(&h, "top");
    h.state_mut().session.execute("prop.set", json!({"layer": cam, "path": "transform/position", "value": [480, 180, -300]})).unwrap();
    settle(&mut h);
    let after = centre(&h, "top");
    assert!(after.0 - before.0 > 10.0, "the camera moves right in the Top view: {before:?} -> {after:?}");
    assert!((after.1 - before.1).abs() < 1.0, "{before:?} -> {after:?}");
}

/// A 4-view layout over a comp with a 3D solid and a camera.
fn four_views() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.newCamera", json!({"position": [320, 180, -300], "poi": [320, 180, 0]})).unwrap();
    s.execute("view.layout", json!({"views": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    h
}

/// #670: every pane of a multi-view layout has its own view selector; choosing a view changes
/// that pane (its camera and its wireframes) and no other, and it is remembered for the comp.
#[test]
fn each_layout_pane_chooses_its_own_view() {
    let mut h = four_views();
    let present = |h: &Harness<'_, EffectcraftApp>, view: &str| h.state().auto.find(&format!("viewer.view.{view}")).is_some();
    let has_camera = |h: &Harness<'_, EffectcraftApp>, view: &str| h.state().auto.query(&format!("viewer.view.{view}.camera.")).len() == 1;
    for p in 1..=3 {
        assert!(h.state().auto.find(&format!("viewer.view.{p}.type")).is_some(), "pane {p} has a selector");
    }
    assert!(present(&h, "front") && has_camera(&h, "front"));
    // Pane 2 (the Front view) becomes the Left view.
    let left = View3D::ALL.iter().position(|v| *v == View3D::Left).unwrap();
    click_id(&mut h, "viewer.view.2.type");
    click_id(&mut h, &format!("viewer.view.2.typeItem.{left}"));
    settle(&mut h);
    assert_eq!(h.state().auto.find("viewer.view.2.type").unwrap().label, "View of pane");
    assert!(present(&h, "left") && has_camera(&h, "left"), "pane 2 shows the Left view with its wireframe");
    assert!(!present(&h, "front"), "pane 2 no longer shows Front");
    assert!(present(&h, "top") && present(&h, "right"), "the other panes keep their views");
    let cid = h.state().session.active_comp_id().unwrap();
    assert_eq!(h.state().session.state.views3d[&cid].pane(1), View3D::Left);
    // The Active Camera view of a pane does not draw the camera it looks through.
    h.state_mut().session.execute("view.setPaneView", json!({"pane": 1, "view": "activeCamera"})).unwrap();
    settle(&mut h);
    assert!(present(&h, "activeCamera") && !has_camera(&h, "activeCamera"));
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

/// A selected 3D solid (`w` x `h`) in an empty 640x360 comp seen by the default camera.
fn gizmo_harness(w: u32, h: u32) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": w, "height": h})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.select", json!({"layers": [solid]})).unwrap();
    let mut harness = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut harness);
    (harness, solid)
}

fn transform(h: &mut Harness<'_, EffectcraftApp>, layer: u64, prop: &str) -> Vec<f64> {
    let v = h.state_mut().session.execute("prop.get", json!({"layer": layer, "path": format!("transform/{prop}")})).unwrap()["value"].clone();
    serde_json::from_value(v).unwrap()
}

/// The centre of the automation element `id`.
fn centre(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Pos2 {
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    egui::pos2(x + w / 2.0, y + hh / 2.0)
}

/// #1618: the gizmo's arrows drag a 3D layer along one axis. Z, which points at the default
/// camera, still has a grabbable arrow and pushes the layer in depth, as one undo step.
#[test]
fn gizmo_arrows_move_a_3d_layer_along_one_axis() {
    let (mut h, solid) = gizmo_harness(200, 200);
    let p0 = transform(&mut h, solid, "position");

    let z = centre(&h, &format!("viewer.gizmo.{solid}.z"));
    drag(&mut h, z, z + egui::vec2(30.0, -30.0));
    let p1 = transform(&mut h, solid, "position");
    assert_eq!(&p1[..2], &p0[..2], "x and y stay put");
    assert!(p1[2] > p0[2] + 10.0, "pushed back in depth: {p0:?} -> {p1:?}");

    settle(&mut h);
    let x = centre(&h, &format!("viewer.gizmo.{solid}.x"));
    drag(&mut h, x, x + egui::vec2(40.0, 25.0));
    let p2 = transform(&mut h, solid, "position");
    assert!(p2[0] > p1[0] + 10.0, "moved along x: {p1:?} -> {p2:?}");
    assert_eq!(&p2[1..], &p1[1..], "only x changes");

    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(transform(&mut h, solid, "position"), p1, "the x drag is one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(transform(&mut h, solid, "position"), p0, "the z drag is one undo step");
}

/// #1627 review: an arrow drawn over a box handle is what the pointer grabs (it scaled the
/// layer). At 100% a 120 px wide layer has its right-edge handle under the X arrow's tip.
#[test]
fn a_gizmo_arrow_wins_over_the_box_handle_under_it() {
    let (mut h, solid) = gizmo_harness(120, 200);
    h.state_mut().ui.viewer.zoom = Some(1.0);
    settle(&mut h);
    let x = centre(&h, &format!("viewer.gizmo.{solid}.x"));
    assert!(x.distance(centre(&h, &format!("viewer.handle.{solid}.5"))) < 2.0, "the right-edge handle is under the X tip");

    h.event(egui::Event::PointerMoved(x));
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Move, "hover shows the move cursor, not resize");

    let p0 = transform(&mut h, solid, "position");
    drag(&mut h, x, x + egui::vec2(30.0, 0.0));
    let p1 = transform(&mut h, solid, "position");
    assert_eq!(transform(&mut h, solid, "scale"), vec![100.0, 100.0, 100.0], "not scaled");
    assert!((p1[0] - p0[0] - 30.0).abs() < 1e-6, "moved 30 px along x: {p0:?} -> {p1:?}");
    assert_eq!(&p1[1..], &p0[1..]);
}
