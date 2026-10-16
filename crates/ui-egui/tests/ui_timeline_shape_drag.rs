//! Real pointer drags reorder shape groups and move paints into nested Contents.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn setup() -> (Harness<'static, EffectcraftApp>, u64, u64, u64, u64) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Shapes", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = session.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let contents = session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.sub("contents").unwrap();
    let root = contents.uid;
    let group = contents.groups().next().unwrap().uid;
    let nested = contents.groups().next().unwrap().sub("contents").unwrap().uid;
    let stroke = session.execute("layer.addShapeItem", json!({"layer": layer, "kind": "stroke"})).unwrap()["uid"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1100.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.extend([root, group]);
    h.run_steps(3);
    (h, layer, root, nested, stroke)
}

fn props<'a>(h: &'a Harness<'_, EffectcraftApp>, layer: u64) -> &'a PropGroup {
    &h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props
}

fn element(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Rect {
    let id = format!("timeline.group.{uid}.name");
    let e = h.state().auto.find(&id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn begin_drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
}

fn release(h: &mut Harness<'_, EffectcraftApp>, to: Pos2) {
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    begin_drag(h, from, to);
    release(h, to);
}

fn project(h: &Harness<'_, EffectcraftApp>) -> Value {
    serde_json::to_value(h.state().session.project.as_ref()).unwrap()
}

#[test]
fn stroke_dropped_on_nested_contents_moves_inside_and_undo_restores_it() {
    let (mut h, layer, root, nested, stroke) = setup();
    let before = project(&h);
    let history = h.state().session.history.undo.len();
    let from = element(&h, stroke).center();
    let to = element(&h, nested).center();
    drag(&mut h, from, to);
    assert!(!props(&h, layer).find_group(root).unwrap().children.iter().any(|n| n.uid() == stroke), "Stroke leaves layer Contents");
    assert!(props(&h, layer).find_group(nested).unwrap().children.iter().any(|n| n.uid() == stroke), "Stroke enters the shape group's Contents");
    assert!(h.state().ui.timeline.open_groups.contains(&nested), "destination opens to show the result");
    assert_eq!(h.state().session.history.undo.len(), history + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(project(&h), before);
}

#[test]
fn group_dropped_on_another_groups_edge_reorders_the_groups() {
    let (mut h, layer, root, _, _) = setup();
    let first = props(&h, layer).find_group(root).unwrap().groups().next().unwrap().uid;
    h.state_mut().ui.timeline.open_groups.remove(&first);
    let other = h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer, "kind": "group"})).unwrap()["uid"].as_u64().unwrap();
    h.run_steps(3);
    let from = element(&h, other).center();
    let row = element(&h, first);
    drag(&mut h, from, pos2(row.center().x, row.min.y + 1.0));
    let order: Vec<u64> = props(&h, layer).find_group(root).unwrap().groups().map(|g| g.uid).collect();
    assert_eq!(order.first(), Some(&other));
    assert_eq!(order.get(1), Some(&first));
    assert!(props(&h, layer).find_group(other).unwrap().sub("contents").unwrap().children.is_empty(), "an edge drop reorders rather than nests");
    let from = element(&h, other).center();
    let row = element(&h, first);
    drag(&mut h, from, pos2(row.center().x, row.max.y - 1.0));
    let order: Vec<u64> = props(&h, layer).find_group(root).unwrap().groups().map(|g| g.uid).collect();
    assert_eq!(order.first(), Some(&first), "the forward move handles removal before insertion");
    assert_eq!(order.get(1), Some(&other));
}

#[test]
fn dropping_a_group_on_its_own_contents_is_rejected() {
    let (mut h, layer, root, nested, _) = setup();
    let group = props(&h, layer).find_group(root).unwrap().groups().next().unwrap().uid;
    let before = project(&h);
    let history = h.state().session.history.undo.len();
    let from = element(&h, group).center();
    let to = element(&h, nested).center();
    drag(&mut h, from, to);
    assert_eq!(project(&h), before);
    assert_eq!(h.state().session.history.undo.len(), history);
}

#[test]
fn selected_paints_move_together_into_contents() {
    let (mut h, layer, root, nested, stroke) = setup();
    let fill = h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer, "kind": "fill"})).unwrap()["uid"].as_u64().unwrap();
    h.state_mut().session.execute("prop.select", json!({"layer": layer, "prop": stroke, "selectKeys": false})).unwrap();
    h.state_mut().session.execute("prop.select", json!({"layer": layer, "prop": fill, "add": true, "selectKeys": false})).unwrap();
    h.run_steps(3);
    let from = element(&h, stroke).center();
    let to = element(&h, nested).center();
    drag(&mut h, from, to);
    let remaining: Vec<u64> = props(&h, layer).find_group(root).unwrap().children.iter().map(|n| n.uid()).collect();
    assert!(!remaining.contains(&stroke) && !remaining.contains(&fill));
    let moved: Vec<u64> = props(&h, layer).find_group(nested).unwrap().children.iter().map(|n| n.uid()).collect();
    assert_eq!(&moved[moved.len() - 2..], &[stroke, fill]);
}

#[test]
fn locked_shape_items_cannot_start_a_drag() {
    let (mut h, layer, _, nested, stroke) = setup();
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
    h.run_steps(3);
    let before = project(&h);
    let from = element(&h, stroke).center();
    let to = element(&h, nested).center();
    drag(&mut h, from, to);
    assert_eq!(project(&h), before);
    assert!(h.state().auto.find(&format!("timeline.group.{stroke}.drag")).is_none());
}

#[test]
fn a_drop_into_a_collapsed_shape_group_opens_the_group_and_its_contents() {
    let (mut h, layer, root, nested, stroke) = setup();
    let group = props(&h, layer).find_group(root).unwrap().groups().next().unwrap().uid;
    h.state_mut().ui.timeline.open_groups.remove(&group);
    h.run_steps(3);
    let from = element(&h, stroke).center();
    let to = element(&h, group).center();
    drag(&mut h, from, to);
    assert!(props(&h, layer).find_group(nested).unwrap().children.iter().any(|n| n.uid() == stroke));
    assert!(h.state().ui.timeline.open_groups.contains(&group));
    assert!(h.state().ui.timeline.open_groups.contains(&nested));
    assert!(h.state().auto.find(&format!("timeline.group.{stroke}.name")).is_some(), "the moved item remains visible");
}

#[test]
fn escape_cancels_a_shape_drag_without_an_undo_step() {
    let (mut h, _, _, nested, stroke) = setup();
    let before = project(&h);
    let history = h.state().session.history.undo.len();
    let from = element(&h, stroke).center();
    let to = element(&h, nested).center();
    begin_drag(&mut h, from, to);
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    release(&mut h, to);
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(project(&h), before);
    assert_eq!(h.state().session.history.undo.len(), history);
}
