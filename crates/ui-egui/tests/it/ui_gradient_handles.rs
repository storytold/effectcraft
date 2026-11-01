//! #612: a shape layer's Gradient Fill start and end points are dragged in the Composition viewer,
//! as in After Effects. With the gradient (or a property in it) selected, `viewer.gradientPoint.*`
//! handles sit on the points, in the space of the group holding the gradient; a drag sets the
//! point in one undo step and doesn't move the layer. Uses only APIs that existed before the
//! handles, so it runs (and fails) on the code without them.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// The Gradient Fill group in shape contents `g`, through nested groups.
fn gfill(g: &PropGroup) -> Option<&PropGroup> {
    for x in g.groups() {
        if x.match_id == "gfill" {
            return Some(x);
        }
        if let Some(found) = gfill(x) {
            return Some(found);
        }
    }
    None
}

struct Ids {
    layer: u64,
    gfill: u64,
    start: u64,
    end: u64,
}

/// A 640×360 comp with a rectangle shape layer, a Gradient Fill added inside the rectangle's group
/// (`in_group`) or at the top of its contents, after `prep` runs on the session.
fn setup(in_group: bool, prep: impl FnOnce(&mut Session, u64, u64)) -> (Session, Ids) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "duration": 4})).unwrap();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let group = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("contents").unwrap().groups().next().unwrap().uid;
    let mut p = json!({"layer": l, "kind": "gfill"});
    if in_group {
        p["group"] = json!(group);
    }
    s.execute("layer.addShapeItem", p).unwrap();
    prep(&mut s, l, group);
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap();
    let g = gfill(&layer.props).expect("a Gradient Fill");
    let ids = Ids { layer: l, gfill: g.uid, start: g.get("start").unwrap().uid, end: g.get("end").unwrap().uid };
    s.state.selected_layers = vec![LayerId(l)];
    (s, ids)
}

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    h
}

fn handle(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Option<Pos2> {
    h.state().auto.find(&format!("viewer.gradientPoint.{uid}")).map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
}

fn point(h: &Harness<'_, EffectcraftApp>, ids: &Ids, uid: u64) -> [f64; 2] {
    h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.find(uid).unwrap().value.as_vec2()
}

/// Screen pixels per comp pixel.
fn zoom(h: &Harness<'_, EffectcraftApp>) -> f32 {
    let a = comp_to_screen(&h.ctx, [0.0, 0.0]).unwrap();
    let b = comp_to_screen(&h.ctx, [100.0, 0.0]).unwrap();
    (b.x - a.x) / 100.0
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

/// With the Gradient Fill selected, its End Point is a handle on the point; dragging it moves the
/// point by the drag (in comp pixels, the layer and group being unscaled), as one undo step,
/// without moving the layer.
#[test]
fn dragging_the_end_point_moves_it_in_one_undo_step() {
    let (mut s, ids) = setup(false, |_, _, _| {});
    s.state.selected_props = vec![(LayerId(ids.layer), ids.gfill)];
    let mut h = harness(s);
    let at = handle(&h, ids.end).unwrap_or_else(|| panic!("no viewer.gradientPoint.{}: the Gradient Fill's End Point isn't draggable in the viewer", ids.end));
    assert!(handle(&h, ids.start).is_some(), "and its Start Point");
    let (end, start) = (point(&h, &ids, ids.end), point(&h, &ids, ids.start));
    let position = h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop("transform/position").unwrap().value.clone();
    let undo = h.state().session.history.undo.len();
    let z = zoom(&h);
    drag(&mut h, at, at + vec2(60.0, 30.0) * z);
    let moved = point(&h, &ids, ids.end);
    assert!(near(moved, [end[0] + 60.0, end[1] + 30.0], 1.5), "End Point {end:?} moved by the drag: {moved:?}");
    assert_eq!(point(&h, &ids, ids.start), start, "the Start Point stays");
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().clone();
    assert_eq!(layer.props.prop("transform/position").unwrap().value, position, "the layer didn't move");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "the drag is one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(point(&h, &ids, ids.end), end, "one undo puts it back");
}

/// A gradient inside a moved and scaled group: its handles sit where the group draws the points,
/// and a drag converts back into the group's space (half as far at 200%).
#[test]
fn points_follow_the_group_transform() {
    let (mut s, ids) = setup(true, |s, l, group| {
        let tr = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find_group(group).unwrap().sub("transform").unwrap().clone();
        s.execute("prop.set", json!({"layer": l, "prop": tr.get("scale").unwrap().uid, "value": [200.0, 200.0]})).unwrap();
        s.execute("prop.set", json!({"layer": l, "prop": tr.get("position").unwrap().uid, "value": [40.0, -20.0]})).unwrap();
    });
    s.state.selected_props = vec![(LayerId(ids.layer), ids.start)];
    let mut h = harness(s);
    // Layer at the comp's centre (320, 180); group: position (40, -20), scale 200%.
    let start = point(&h, &ids, ids.start);
    let want = comp_to_screen(&h.ctx, [(320.0 + 40.0 + 2.0 * start[0]) as f32, (180.0 - 20.0 + 2.0 * start[1]) as f32]).unwrap();
    let at = handle(&h, ids.start).expect("a Start Point handle (selecting a property in the gradient shows it)");
    assert!((at - want).length() < 1.5, "the handle is where the group draws the point: {at:?} vs {want:?}");
    let z = zoom(&h);
    drag(&mut h, at, at + vec2(40.0, 20.0) * z);
    let moved = point(&h, &ids, ids.start);
    assert!(near(moved, [start[0] + 20.0, start[1] + 10.0], 1.0), "in group space, half the comp drag: {start:?} → {moved:?}");
}

/// No handles unless the gradient (or a property in it) is selected, or with layer controls off.
#[test]
fn handles_show_only_for_a_selected_gradient() {
    let (s, ids) = setup(false, |_, _, _| {});
    let h = harness(s);
    assert!(handle(&h, ids.end).is_none(), "only the layer is selected");
    let (mut s, ids) = setup(false, |_, _, _| {});
    let position = s.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop("transform/position").unwrap().uid;
    s.state.selected_props = vec![(LayerId(ids.layer), position)];
    let h = harness(s);
    assert!(handle(&h, ids.end).is_none(), "another property is selected");
    let (mut s, ids) = setup(false, |_, _, _| {});
    s.state.selected_props = vec![(LayerId(ids.layer), ids.gfill)];
    let mut h = harness(s);
    assert!(handle(&h, ids.end).is_some());
    h.state_mut().ui.viewer.show_layer_controls = false;
    h.run_steps(3);
    assert!(handle(&h, ids.end).is_none(), "View ▸ Show Layer Controls off hides them");
}
