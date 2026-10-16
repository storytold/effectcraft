//! Add follows the selected shape Contents, including nested groups.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn element(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn setup() -> (Harness<'static, EffectcraftApp>, u64, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = s.execute("layer.newShape", json!({"kind": "rect", "strokeWidth": 2})).unwrap()["layer"].as_u64().unwrap();
    let contents = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.sub("contents").unwrap();
    let root = contents.uid;
    let group = contents.groups().next().unwrap();
    let group_uid = group.uid;
    let nested = group.sub("contents").unwrap().uid;
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.extend([root, group_uid, nested]);
    h.run_steps(3);
    (h, layer, root, nested)
}

fn add(h: &mut Harness<'_, EffectcraftApp>, contents: u64, label: &str) {
    let at = element(h, &format!("timeline.group.{contents}.add"));
    click(h, at);
    let item = h.get_by_label(label).rect().center();
    click(h, item);
}

#[test]
fn root_add_puts_stroke_into_selected_nested_contents_and_undo_restores_it() {
    let (mut h, layer, root, nested) = setup();
    let at = element(&h, &format!("timeline.group.{nested}.name"));
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_props, [(LayerId(layer), nested)]);
    add(&mut h, root, "Stroke");
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    assert_eq!(l.props.sub("contents").unwrap().groups().count(), 1, "no stroke outside the group");
    let c = l.props.find_group(nested).unwrap();
    assert_eq!(c.groups().filter(|g| g.match_id == "stroke").count(), 2, "new stroke joins the rectangle's existing stroke");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    assert_eq!(l.props.find_group(nested).unwrap().groups().filter(|g| g.match_id == "stroke").count(), 1);
}

#[test]
fn nested_add_targets_its_contents_even_when_root_contents_is_selected() {
    let (mut h, layer, root, nested) = setup();
    let at = element(&h, &format!("timeline.group.{root}.name"));
    click(&mut h, at);
    add(&mut h, nested, "Trim Paths");
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    assert_eq!(l.props.sub("contents").unwrap().groups().count(), 1);
    assert!(l.props.find_group(nested).unwrap().groups().any(|g| g.match_id == "trim"));
}

#[test]
fn root_add_follows_selected_path_property_into_its_contents() {
    let (mut h, layer, root, nested) = setup();
    let path = h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer, "kind": "path", "group": nested})).unwrap()["uid"].as_u64().unwrap();
    let prop = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(path).unwrap().get("path").unwrap().uid;
    h.state_mut().session.execute("prop.select", json!({"layer": layer, "prop": prop, "selectKeys": false})).unwrap();
    h.run_steps(3);
    add(&mut h, root, "Fill");
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    assert_eq!(l.props.sub("contents").unwrap().groups().count(), 1);
    assert_eq!(l.props.find_group(nested).unwrap().groups().filter(|g| g.match_id == "fill").count(), 2);
}
