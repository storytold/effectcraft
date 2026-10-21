//! #612: a gradient property's "Edit Gradient…" in the Timeline opens an editor. Before the fix
//! the value only painted its label: no automation id, nothing to click. This test uses only APIs
//! that existed before the Gradient Editor, so it runs (and fails) on the code without it; the
//! editor itself is tested in `ui_gradient_editor`.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// The uids from the layer's root down to the Gradient Fill group, and its Colors property.
fn gradient_fill(g: &PropGroup) -> Option<(Vec<u64>, u64)> {
    for x in g.groups() {
        if x.match_id == "gfill" {
            return Some((vec![x.uid], x.get("colors")?.uid));
        }
        if let Some((mut path, colors)) = gradient_fill(x) {
            path.insert(0, x.uid);
            return Some((path, colors));
        }
    }
    None
}

#[test]
fn the_timeline_gradient_value_opens_an_editor() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addShapeItem", json!({"layer": l, "kind": "gfill"})).unwrap();
    let (groups, colors) = gradient_fill(&s.active_comp().unwrap().layer(LayerId(l)).unwrap().props).expect("a Gradient Fill");
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.open_groups.extend(groups);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    let id = format!("timeline.prop.{colors}.value");
    let r = h.state().auto.find(&id).unwrap_or_else(|| panic!("no automation id {id}: the Timeline's Edit Gradient… isn't clickable")).rect;
    let p = pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    assert!(h.state().dialog.is_some(), "clicking Edit Gradient… opens an editor");
}
