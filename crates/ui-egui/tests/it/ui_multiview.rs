//! #3845: every pane of a 2- or 4-view layout takes the press. A press in an inactive pane makes
//! it the active pane, and the same press selects or drags there, through that pane's view (as in
//! After Effects). The side panes draw frames the background renderer made.

use effectcraft_engine::Session;
use effectcraft_engine::render::three_d::{View3D, Views3D};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use serde_json::json;

/// A `layout`-view (1, 2 or 4) viewer over a 640×360 comp with one selected 200×200 3D solid at
/// the centre and a camera at `[320, 180, -300]` looking at it. Returns the harness and the
/// solid's id.
fn harness(layout: u8) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.newCamera", json!({"position": [320, 180, -300], "poi": [320, 180, 0]})).unwrap();
    s.execute("layer.select", json!({"layers": [solid]})).unwrap();
    s.execute("view.layout", json!({"views": layout})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    (h, solid)
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

/// The centre of the automation element `id`.
fn centre(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    egui::pos2(x + w / 2.0, y + hh / 2.0)
}

/// Click the centre of the automation element `id`.
fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = centre(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// The active comp's 3D views (the view of each position, the active position).
fn views(h: &Harness<'_, EffectcraftApp>) -> Views3D {
    let s = &h.state().session;
    s.active_comp_id().and_then(|c| s.state.views3d.get(&c).cloned()).unwrap_or_default()
}

/// The value of `transform/{prop}` of `layer`.
fn pos3(h: &mut Harness<'_, EffectcraftApp>, layer: u64, prop: &str) -> Vec<f64> {
    let v = h.state_mut().session.execute("prop.get", json!({"layer": layer, "path": format!("transform/{prop}")})).unwrap()["value"].clone();
    serde_json::from_value(v).unwrap()
}

/// A point of pane `pos` on its pasteboard: below the pane's view selector and never on a layer.
fn empty_point(h: &Harness<'_, EffectcraftApp>, pos: usize) -> Pos2 {
    let id = format!("viewer.pane.{pos}");
    let [x, y, ..] = h.state().auto.find(&id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    egui::pos2(x + 30.0, y + 40.0)
}

/// Drag `button` from `from` to `to` in 8 steps, each event a frame of its own.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, button: PointerButton) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.run_steps(3);
}

/// The layer under the press in the Top pane is dragged on the Top view's plane (X and Z), in the
/// press that makes the pane active, as one undo step.
#[test]
fn a_press_in_a_side_pane_activates_it_and_drags_in_the_same_press() {
    let (mut h, solid) = harness(4);
    // A floor layer, so the Top pane sees its face, not its edge.
    h.state_mut().session.execute("prop.set", json!({"layer": solid, "path": "transform/rotationX", "value": 90.0})).unwrap();
    settle(&mut h);
    // The layer sits at the comp's centre, which the Top pane shows in its middle.
    let a = centre(&h, "viewer.view.top");
    let p0 = pos3(&mut h, solid, "position");
    drag(&mut h, a, a + egui::vec2(40.0, 30.0), PointerButton::Primary);
    assert_eq!(views(&h).active(), 0, "the Top pane became active");
    let p1 = pos3(&mut h, solid, "position");
    assert!((p1[0] - p0[0]).abs() > 5.0 && (p1[2] - p0[2]).abs() > 5.0, "moved in X and Z: {p0:?} -> {p1:?}");
    assert!((p1[1] - p0[1]).abs() < 1e-9, "Y unchanged");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(pos3(&mut h, solid, "position"), p0, "one undo step");
}

/// Activating a pane moves no view; a middle-click activates without changing the selection, and
/// the View menu's 3D views then act on the active pane.
#[test]
fn activating_keeps_views_in_place_and_middle_click_keeps_the_selection() {
    let (mut h, solid) = harness(4);
    let before = [0, 1, 2, 3].map(|p| views(&h).view_at(p));
    let empty = empty_point(&h, 1);
    drag(&mut h, empty, empty, PointerButton::Middle);
    assert_eq!(views(&h).active(), 1);
    assert_eq!([0, 1, 2, 3].map(|p| views(&h).view_at(p)), before);
    assert_eq!(h.state().session.state.selected_layers, vec![effectcraft_engine::project::LayerId(solid)]);
    h.state_mut().session.execute("view.3d.custom2", json!({})).unwrap();
    assert_eq!(views(&h).view_at(1), View3D::Custom2, "the View menu acts on the active pane");
    h.run_steps(2);
    assert!(h.state().auto.find("viewer.view.4.type").is_some(), "the bottom-right pane, now a side pane, has a view selector");
}

#[test]
fn each_position_keeps_its_own_picture_zoom() {
    let (mut h, _) = harness(4);
    h.state_mut().ui.viewer.zoom = Some(0.8);
    h.run_steps(2);
    let c = empty_point(&h, 0);
    drag(&mut h, c, c, PointerButton::Middle);
    assert_eq!(h.state().ui.viewer.zoom, None, "position 0 starts fitted");
    let c = empty_point(&h, 3);
    drag(&mut h, c, c, PointerButton::Middle);
    assert_eq!(h.state().ui.viewer.zoom, Some(0.8), "position 3 got its zoom back");
    // Another comp, then back: each position's framing survives the round trip.
    let first = h.state().session.active_comp_id().unwrap();
    h.state_mut().session.execute("comp.new", json!({"name": "B", "width": 640, "height": 360, "duration": 2})).unwrap();
    h.run_steps(3);
    h.state_mut().session.execute("comp.open", json!({"comp": first.0})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().ui.viewer.zoom, Some(0.8));
}

#[test]
fn the_locked_viewer_never_activates_a_pane() {
    let (mut h, _) = harness(4);
    h.state_mut().session.execute("view.splitLockedViewer", json!({})).unwrap();
    settle(&mut h);
    let locked = h.state().auto.find("viewer.locked").map(|e| e.rect).unwrap_or_else(|| panic!("the locked viewer registers viewer.locked"));
    let inside = egui::pos2(locked[0] + 20.0, locked[1] + locked[3] / 2.0);
    drag(&mut h, inside, inside, PointerButton::Primary);
    assert_eq!(views(&h).active(), 3);
    for p in 0..4 {
        let r = h.state().auto.find(&format!("viewer.pane.{p}")).unwrap().rect;
        assert!(r[0] >= locked[0] + locked[2], "pane {p} sits right of the locked viewer");
    }
}

/// A side pane's view selector (in its corner) and the selector's popup (over the pane) choose the
/// pane's view and leave the active pane alone.
#[test]
fn the_view_selector_of_a_side_pane_does_not_activate_it() {
    let (mut h, _) = harness(4);
    let left = View3D::ALL.iter().position(|v| *v == View3D::Left).unwrap();
    // Position 0's selector (the selectors number the panes from 1).
    click_id(&mut h, "viewer.view.1.type");
    click_id(&mut h, &format!("viewer.view.1.typeItem.{left}"));
    settle(&mut h);
    assert_eq!(views(&h).active(), 3, "still the bottom-right pane");
    assert_eq!(views(&h).view_at(0), View3D::Left);
}

/// A second button pressed while a drag holds the first one belongs to that drag: it does not
/// switch the pane under the drag. (One event per pass, so the second press is a press of its own
/// pass, after the drag has started.)
#[test]
fn a_second_button_during_a_drag_does_not_activate_another_pane() {
    let (mut h, _) = harness(4);
    let (from, to) = (empty_point(&h, 3), empty_point(&h, 0));
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(views(&h).active(), 3, "the dragging pane stays active");
    h.event(Event::PointerButton { pos: to, button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(views(&h).active(), 3);
}

/// An active position the layout does not show (editor state set directly, which the layout
/// commands never leave) shows that pane alone instead of panes stacked under it.
#[test]
fn an_active_position_the_layout_hides_shows_alone() {
    let (mut h, _) = harness(4);
    h.state_mut().session.execute("view.activatePane", json!({"position": 1})).unwrap();
    h.state_mut().session.state.view_layout = 2;
    settle(&mut h);
    assert!(h.state().auto.find("viewer.pane.1").is_some(), "the active pane is shown");
    for p in [0, 3] {
        assert!(h.state().auto.find(&format!("viewer.pane.{p}")).is_none(), "pane {p} is not drawn under it");
    }
}

/// In a comp with no 3D layer the side panes stay pictures, as before: a press there activates
/// nothing and does nothing, and there are no layer controls to aim at.
#[test]
fn in_a_2d_comp_the_side_panes_stay_passive() {
    let (mut h, solid) = harness(4);
    let cam = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.is_camera()).unwrap().id.0;
    h.state_mut().session.execute("edit.clear", json!({"layers": [cam]})).unwrap();
    h.state_mut().session.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": false})).unwrap();
    // (Clearing the camera cleared the selection.)
    h.state_mut().session.execute("layer.select", json!({"layers": [solid]})).unwrap();
    settle(&mut h);
    assert!(!h.state().session.active_comp().unwrap().has_3d());
    let selected = vec![effectcraft_engine::project::LayerId(solid)];
    assert_eq!(h.state().session.state.selected_layers, selected);
    assert!(h.state().auto.find(&format!("viewer.anchor.{solid}")).is_some(), "the active pane shows the layer's controls");
    let empty = empty_point(&h, 0);
    drag(&mut h, empty, empty, PointerButton::Primary);
    assert_eq!(views(&h).active(), 3, "the bottom-right pane stays the one that takes input");
    assert_eq!(h.state().session.state.selected_layers, selected, "the press did nothing (a click there would deselect)");
    assert!(h.state().auto.find(&format!("viewer.pane.0.anchor.{solid}")).is_none(), "no layer controls in a passive pane");
}

/// With View ▸ Show Rulers on, a side pane is framed on the same area the rulers leave the active
/// pane, so the axis arrow aimed at in a side pane is the arrow the same press grabs.
#[test]
fn with_rulers_on_the_arrow_aimed_at_in_a_side_pane_is_grabbed() {
    let (mut h, solid) = harness(4);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "view.rulers", json!({"value": true})).unwrap();
    // A floor layer, so the Top pane sees its face.
    h.state_mut().session.execute("prop.set", json!({"layer": solid, "path": "transform/rotationX", "value": 90.0})).unwrap();
    settle(&mut h);
    assert!(h.state().ui.viewer.rulers);
    let x = centre(&h, &format!("viewer.pane.0.gizmo.{solid}.x"));
    let p0 = pos3(&mut h, solid, "position");
    // Along the arrow and a little across it: the arrow keeps the move on X, a free drag would not.
    drag(&mut h, x, x + egui::vec2(40.0, 12.0), PointerButton::Primary);
    assert_eq!(views(&h).active(), 0, "the Top pane became active");
    let p1 = pos3(&mut h, solid, "position");
    assert!((p1[0] - p0[0]).abs() > 5.0, "moved along X: {p0:?} -> {p1:?}");
    assert!((p1[1] - p0[1]).abs() < 1e-9 && (p1[2] - p0[2]).abs() < 1e-9, "only X changed: {p0:?} -> {p1:?}");
}

/// A selected layer outside a side pane's picture registers no controls there: their rectangles
/// would lie over the neighbouring pane, where a press is that pane's.
#[test]
fn a_side_pane_registers_only_the_controls_it_shows() {
    let (mut h, solid) = harness(4);
    for p in 0..3 {
        assert!(h.state().auto.find(&format!("viewer.pane.{p}.anchor.{solid}")).is_some(), "pane {p} shows the anchor point");
    }
    // The Top pane's picture panned a long way right: the layer is over the Front pane's area.
    h.state_mut().ui.viewer.pane_frames[0] = effectcraft_ui_egui::state::PaneFrame { zoom: Some(1.0), pan: [1200.0, 0.0] };
    settle(&mut h);
    assert!(h.state().auto.find(&format!("viewer.pane.0.anchor.{solid}")).is_none(), "the Top pane's anchor point is out of view");
    for axis in ["x", "y", "z"] {
        assert!(h.state().auto.find(&format!("viewer.pane.0.gizmo.{solid}.{axis}")).is_none(), "the Top pane's {axis} arrow is out of view");
    }
    assert!(h.state().auto.find(&format!("viewer.pane.1.anchor.{solid}")).is_some(), "the Front pane still shows its own");
}

/// The textures drawn in the last pass, with the rectangles they cover.
fn images(h: &Harness<'_, EffectcraftApp>) -> Vec<(egui::TextureId, egui::Rect)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(egui::TextureId, egui::Rect)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => out.push((m.texture_id, m.calc_bounds())),
            _ => {}
        }
    }
    let mut out = vec![];
    for c in &h.output().shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// The rectangle of the automation element `id`.
fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, hh))
}

/// A pane that has just become active never shows the previously active pane's frame (another
/// view): until its own frame has come, its side-pane picture stands in.
#[test]
fn a_newly_active_pane_never_shows_the_previous_panes_frame() {
    let (mut h, _) = harness(4);
    let (top, main) = (rect_of(&h, "viewer.pane.0"), rect_of(&h, "viewer.pane.3"));
    let drawn = images(&h);
    let in_rect = |r: egui::Rect| drawn.iter().filter(|(_, b)| r.contains_rect(*b)).map(|(t, _)| *t).collect::<Vec<_>>();
    let main_frame = *in_rect(main).first().unwrap_or_else(|| panic!("the active pane draws its frame"));
    let top_picture = *in_rect(top).first().unwrap_or_else(|| panic!("the Top pane draws its picture"));
    // Press in the Top pane: in that pass it is the active pane and its own frame is not there yet.
    let at = empty_point(&h, 0);
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(Event::PointerButton { pos: at, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(views(&h).active(), 0);
    let now: Vec<egui::TextureId> = images(&h).into_iter().filter(|(_, b)| top.contains_rect(*b)).map(|(t, _)| t).collect();
    assert!(!now.contains(&main_frame), "the Top pane does not show the Active Camera pane's frame: {now:?} (that frame: {main_frame:?})");
    assert!(now.contains(&top_picture), "it shows its own last picture meanwhile: {now:?}");
    // The bottom-right pane, a side pane now, starts with the frame it showed while active.
    let side: Vec<egui::TextureId> = images(&h).into_iter().filter(|(_, b)| main.contains_rect(*b)).map(|(t, _)| t).collect();
    assert!(side.contains(&main_frame), "the bottom-right pane shows its last frame until its own picture lands: {side:?}");
    h.event(Event::PointerButton { pos: at, button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
    let now: Vec<egui::TextureId> = images(&h).into_iter().filter(|(_, b)| top.contains_rect(*b)).map(|(t, _)| t).collect();
    assert!(!now.is_empty() && !now.contains(&top_picture), "then the viewer's own frame: {now:?}");
}

/// Two panes on the same view at the same magnification want the same frame: the pane made active
/// finds it on screen already, nothing is uploaded again, and it shows that frame rather than its
/// side-pane picture (which stood in until the time or the comp changed). 32 bpc: the half-float
/// frame stays on screen as it is.
#[test]
fn a_pane_made_active_on_the_frame_on_screen_shows_that_frame() {
    let (mut h, _) = harness(2);
    h.state_mut().session.execute("file.projectSettings", json!({"bitDepth": 32})).unwrap();
    h.state_mut().session.execute("view.setPaneView", json!({"position": 0, "view": "activeCamera"})).unwrap();
    settle(&mut h);
    let left = rect_of(&h, "viewer.pane.0");
    let in_left = |h: &Harness<'_, EffectcraftApp>| images(h).into_iter().filter(|(_, b)| left.contains_rect(*b)).map(|(t, _)| t).collect::<Vec<_>>();
    let picture = *in_left(&h).first().unwrap_or_else(|| panic!("the left pane draws its picture"));
    h.state_mut().session.execute("view.activatePane", json!({"position": 0})).unwrap();
    settle(&mut h);
    assert_eq!(views(&h).active(), 0);
    let now = in_left(&h);
    assert!(!now.contains(&picture), "the left pane shows the frame on screen, not its side-pane picture: {now:?}");
}

/// Agents that activate a pane and set the viewer's zoom in one go (no viewer pass in between)
/// zoom the pane they activated, not the one that was active.
#[test]
fn a_zoom_set_right_after_activating_a_pane_is_that_panes() {
    use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
    let (mut h, _) = harness(4);
    let ctx = h.ctx.clone();
    let send = |h: &mut Harness<'_, EffectcraftApp>, method: &str, params: serde_json::Value| {
        let (req, _rx) = ControlRequest::new(method, params);
        let now = h.state().now();
        match control::handle(h.state_mut(), &ctx, &req, now) {
            Outcome::Done(v) => assert_eq!(v["ok"], true, "{method}: {v}"),
            _ => panic!("{method}: no immediate reply"),
        }
    };
    send(&mut h, "engine.execute", json!({"command": "view.activatePane", "params": {"position": 0}}));
    send(&mut h, "ui.set", json!({"viewer": {"zoom": 2.0}}));
    h.run_steps(3);
    assert_eq!(views(&h).active(), 0);
    assert_eq!(h.state().ui.viewer.zoom, Some(2.0), "the Top pane, now active, is at 200%");
    assert_eq!(h.state().ui.viewer.pane_frames[3].zoom, None, "the bottom-right pane keeps its fit");
}

/// With View ▸ Show Rulers on, a press in a side pane's ruler strip (where the rulers are once it
/// is active, to drag guides from) does not activate it.
#[test]
fn a_press_in_a_side_panes_ruler_strip_does_not_activate_it() {
    let (mut h, _) = harness(4);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "view.rulers", json!({"value": true})).unwrap();
    settle(&mut h);
    let top = rect_of(&h, "viewer.pane.0");
    let strip = egui::pos2(top.center().x, top.min.y + 6.0);
    drag(&mut h, strip, strip + egui::vec2(0.0, 80.0), PointerButton::Primary);
    assert_eq!(views(&h).active(), 3, "the bottom-right pane stays active");
    assert!(h.state().session.active_comp().unwrap().guides.is_empty(), "and no guide either");
}

/// A side pane's view menu left open when the pane becomes active is closed: it does not come
/// back open when the pane is a side pane again.
#[test]
fn a_view_menu_left_open_does_not_come_back_open() {
    let (mut h, _) = harness(4);
    click_id(&mut h, "viewer.view.1.type");
    assert!(h.state().auto.find("viewer.view.1.typeItem.0").is_some(), "the Top pane's view menu is open");
    // A press in the Top pane beside the menu makes it the active pane.
    let top = rect_of(&h, "viewer.pane.0");
    let beside = egui::pos2(top.max.x - 30.0, top.center().y);
    drag(&mut h, beside, beside, PointerButton::Middle);
    assert_eq!(views(&h).active(), 0);
    h.state_mut().session.execute("view.activatePane", json!({"position": 3})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find("viewer.view.1.type").is_some(), "the Top pane is a side pane again");
    assert!(h.state().auto.find("viewer.view.1.typeItem.0").is_none(), "with its view menu closed");
}

/// A comp whose 3D layers are gone takes the input in the bottom-right pane again, as a 2D comp
/// does; the pane activated before is active again when 3D comes back.
#[test]
fn a_comp_that_loses_its_3d_layers_takes_input_in_the_bottom_right_pane() {
    let (mut h, solid) = harness(4);
    h.state_mut().session.execute("view.activatePane", json!({"position": 0})).unwrap();
    h.run_steps(3);
    assert!(rect_of(&h, "viewer.pane.0").contains_rect(rect_of(&h, "viewer.area")), "the Top pane takes the input");
    let cam = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.is_camera()).unwrap().id.0;
    h.state_mut().session.execute("edit.clear", json!({"layers": [cam]})).unwrap();
    h.state_mut().session.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": false})).unwrap();
    h.run_steps(3);
    assert!(rect_of(&h, "viewer.pane.3").contains_rect(rect_of(&h, "viewer.area")), "2D: the bottom-right pane takes the input");
    h.state_mut().session.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    h.run_steps(3);
    assert!(rect_of(&h, "viewer.pane.0").contains_rect(rect_of(&h, "viewer.area")), "3D again: the Top pane is active again");
}

/// One view keeps its zoom and pan across comps whatever position each comp has active (framing
/// per position is the 2- and 4-view layouts').
#[test]
fn one_view_keeps_its_framing_across_comps() {
    let (mut h, _) = harness(4);
    h.state_mut().session.execute("view.activatePane", json!({"position": 0})).unwrap();
    h.state_mut().session.execute("view.layout", json!({"views": 1})).unwrap();
    h.run_steps(3);
    h.state_mut().ui.viewer.zoom = Some(0.8);
    h.run_steps(2);
    let first = h.state().session.active_comp_id().unwrap();
    // Comp B, with a 3D layer and the default bottom-right pane active.
    let s = &mut h.state_mut().session;
    s.execute("comp.new", json!({"name": "B", "width": 640, "height": 360, "duration": 2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": l, "switch": "threeD", "value": true})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().ui.viewer.zoom, Some(0.8), "comp B's one view keeps the zoom");
    h.state_mut().session.execute("comp.open", json!({"comp": first.0})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().ui.viewer.zoom, Some(0.8), "and back in the first comp");
}

/// The side panes' frames come from the background renderer's cache, not from a render on the
/// interface thread: a pane that changes view keeps drawing its last frame until the new view's
/// frame has landed.
#[test]
fn side_panes_draw_frames_from_the_cache() {
    use effectcraft_ui_egui::panels::viewer::shown_frame_key;
    let (mut h, _) = harness(4);
    let cid = h.state().session.active_comp_id().unwrap();
    for p in [0, 1, 2] {
        let k = shown_frame_key(&h.ctx, cid, p).unwrap_or_else(|| panic!("pane {p} drew a cached frame"));
        assert!(h.state().frames.is_cached(&k), "pane {p}");
    }
    let old = shown_frame_key(&h.ctx, cid, 0).unwrap();
    h.state_mut().session.execute("view.setPaneView", json!({"position": 0, "view": "left"})).unwrap();
    h.step();
    // The new view's frame was queued in that pass, not rendered there (a worker may have finished
    // it since, so the queue's length is no steady signal).
    assert_eq!(shown_frame_key(&h.ctx, cid, 0), Some(old), "the pane draws its last frame meanwhile");
    settle(&mut h);
    let new = shown_frame_key(&h.ctx, cid, 0).unwrap();
    assert_ne!(new, old, "the pane moved on to the new view's frame once it landed");
    assert!(h.state().frames.is_cached(&new));
}

/// A side pane never draws another comp's frame: after a switch to another comp it draws nothing
/// until that comp's frame has landed, as the active pane does.
#[test]
fn side_panes_never_draw_another_comps_frame() {
    use effectcraft_ui_egui::panels::viewer::shown_frame_key;
    let (mut h, _) = harness(4);
    let first = h.state().session.active_comp_id().unwrap();
    assert!(shown_frame_key(&h.ctx, first, 0).is_some());
    let top = rect_of(&h, "viewer.pane.0");
    let first_picture: Vec<egui::TextureId> = images(&h).into_iter().filter(|(_, b)| top.contains_rect(*b)).map(|(t, _)| t).collect();
    assert!(!first_picture.is_empty(), "the Top pane draws the first comp's picture");
    h.state_mut().session.execute("comp.new", json!({"name": "B", "width": 320, "height": 240, "duration": 2})).unwrap();
    let second = h.state().session.active_comp_id().unwrap();
    for _ in 0..3 {
        h.step();
        // (until the new comp's frame lands; the pane's texture may take it in place after)
        if shown_frame_key(&h.ctx, second, 0).is_none() {
            let now: Vec<egui::TextureId> = images(&h).into_iter().filter(|(_, b)| top.contains_rect(*b)).map(|(t, _)| t).collect();
            assert!(now.iter().all(|t| !first_picture.contains(t)), "pane 0 drew the first comp's picture in the new comp: {now:?}");
        }
    }
    settle(&mut h);
    assert_eq!(shown_frame_key(&h.ctx, second, 0).map(|k| k.comp), Some(second.0), "then the new comp's own frame");
}

/// With a region of interest the viewer's frame covers that region only: when another pane becomes
/// active, the pane that was active does not start with it as its picture (stretched over the
/// whole comp until its own frame lands).
#[test]
fn a_region_of_interest_frame_is_not_handed_to_a_side_pane() {
    let (mut h, _) = harness(4);
    h.state_mut().session.execute("view.setRegionOfInterest", json!({"rect": [100, 50, 200, 100]})).unwrap();
    settle(&mut h);
    let main = rect_of(&h, "viewer.pane.3");
    let in_main = |h: &Harness<'_, EffectcraftApp>| images(h).into_iter().filter(|(_, b)| main.contains_rect(*b)).map(|(t, _)| t).collect::<Vec<_>>();
    let region = *in_main(&h).first().unwrap_or_else(|| panic!("the active pane draws the region's frame"));
    let at = empty_point(&h, 0);
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(Event::PointerButton { pos: at, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(views(&h).active(), 0);
    let side = in_main(&h);
    assert!(!side.contains(&region), "the bottom-right pane does not draw the region's frame as its picture: {side:?}");
    h.event(Event::PointerButton { pos: at, button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
}

/// Wait, with no UI pass, until the frame workers have nothing queued or rendering.
fn idle(h: &Harness<'_, EffectcraftApp>) {
    let start = std::time::Instant::now();
    while h.state().frames.inflight() > 0 {
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "the frame workers finish their frames");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The side panes follow a drag in the active pane, as they did when they rendered on the
/// interface thread: every pass of a drag that moves edits the comp, so a pane that took only the
/// exact frame it asked for kept the picture from before the press until the pointer stopped.
/// The pointer moves on every pass here; between passes every frame asked for lands.
#[test]
fn side_panes_follow_a_drag_in_the_active_pane() {
    use effectcraft_ui_egui::panels::viewer::shown_frame_key;
    let (mut h, solid) = harness(4);
    let cid = h.state().session.active_comp_id().unwrap();
    let before = shown_frame_key(&h.ctx, cid, 0).unwrap_or_else(|| panic!("the Top pane drew a frame"));
    let a = centre(&h, &format!("viewer.anchor.{solid}"));
    h.event(Event::PointerMoved(a));
    h.step();
    h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let mut to = a;
    for _ in 0..24 {
        to += egui::vec2(2.0, 0.0);
        h.event(Event::PointerMoved(to));
        h.step();
        idle(&h);
    }
    let during = shown_frame_key(&h.ctx, cid, 0).unwrap();
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
    assert_ne!(during.content, before.content, "the Top pane showed the layer moving before the release");
    assert_eq!(views(&h).active(), 3);
}

/// A frame a side pane has drawn is not asked for again: it went from the cache (a purge, a full
/// cache), the pane keeps its picture and nothing renders it again.
#[test]
fn a_frame_a_side_pane_drew_is_not_rendered_again() {
    use effectcraft_ui_egui::panels::viewer::shown_frame_key;
    let (mut h, _) = harness(4);
    let cid = h.state().session.active_comp_id().unwrap();
    let k = shown_frame_key(&h.ctx, cid, 0).unwrap_or_else(|| panic!("the Top pane drew a frame"));
    h.state().frames.clear();
    settle(&mut h);
    assert_eq!(shown_frame_key(&h.ctx, cid, 0), Some(k), "the pane keeps its picture");
    assert!(!h.state().frames.is_cached(&k), "and its frame was not rendered again");
}

/// Once a side pane has drawn its frame, the cache keeps it for the pane no more: the pane holds
/// its own picture. (A 4-view layout at rest kept three frames over the RAM cache's budget.)
#[test]
fn a_frame_a_side_pane_drew_leaves_the_cache_as_any_other() {
    use effectcraft_ui_egui::panels::viewer::shown_frame_key;
    let (h, _) = harness(4);
    let cid = h.state().session.active_comp_id().unwrap();
    let k = shown_frame_key(&h.ctx, cid, 0).unwrap_or_else(|| panic!("the Top pane drew a frame"));
    let frames = &h.state().frames;
    assert!(frames.is_cached(&k));
    let budget = frames.budget();
    frames.set_budget(0);
    let kept = frames.is_cached(&k);
    frames.set_budget(budget);
    assert!(!kept, "an empty budget took the Top pane's frame too");
}

/// The interface thread's step with the 4-view layout on the 3D Showcase demo while the time
/// changes every frame (the side panes rendered there before #3845). Run by hand:
/// `cargo test --release -p effectcraft-ui-egui --test it four_view_frame_time -- --ignored --nocapture`.
#[test]
#[ignore = "speed check, run by hand in release"]
fn four_view_frame_time() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("comp.open", json!({"comp": "3D Showcase"})).unwrap();
    s.execute("view.layout", json!({"views": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    settle(&mut h);
    let mut ms = vec![];
    for f in 0..60 {
        h.state_mut().session.execute("time.set", json!({"time": f as f64 / 24.0})).unwrap();
        let t = std::time::Instant::now();
        h.step();
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    println!("4-view step median {:.2} ms, p90 {:.2} ms", ms[30], ms[54]);
}
