//! Shift-clicking a group row in the Timeline adds it to the selection, as on a property row, so
//! several Puppet pins can be selected and deleted together (#1751).

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, u64, [u64; 3]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"width": 100, "height": 60, "color": [0, 0, 1]})).unwrap()["layer"].as_u64().unwrap();
    let pin = |s: &mut Session, x: i64| s.execute("puppet.addPin", json!({"layer": layer, "position": [x, 30]})).unwrap()["pin"].as_u64().unwrap();
    let pins = [pin(&mut s, 10), pin(&mut s, 50), pin(&mut s, 90)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::Timeline);
    app.ui.timeline.open_layers.insert(layer);
    // Twirl open every group down to the pins.
    fn all(g: &PropGroup, out: &mut Vec<u64>) {
        for c in g.groups() {
            out.push(c.uid);
            all(c, out);
        }
    }
    let mut open = vec![];
    if let Some(l) = app.session.active_comp().and_then(|c| c.layer(LayerId(layer))) {
        all(&l.props, &mut open);
    }
    app.ui.timeline.open_groups.extend(open);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1100.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, layer, pins)
}

fn row(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Pos2 {
    let id = format!("timeline.group.{uid}.name");
    let e = h.state().auto.find(&id).unwrap_or_else(|| panic!("no {id}"));
    egui::pos2(e.rect[0] + e.rect[2].min(40.0) / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, uid: u64, modifiers: Modifiers) {
    let at = row(h, uid);
    h.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(3);
    h.input_mut().events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn selected(h: &Harness<'_, EffectcraftApp>, pins: [u64; 3]) -> Vec<u64> {
    h.state().session.state.selected_props.iter().map(|(_, u)| *u).filter(|u| pins.contains(u)).collect()
}

fn pin_count(h: &Harness<'_, EffectcraftApp>, layer: u64) -> usize {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    l.effects().unwrap().groups().map(effectcraft_engine::effects::puppet::pin_count).sum()
}

#[test]
fn shift_click_adds_puppet_pins_and_delete_removes_them_together() {
    let (mut h, layer, pins) = harness();
    let [a, b, c] = pins;
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selected(&h, pins), [a]);
    click(&mut h, b, Modifiers::SHIFT);
    assert_eq!(selected(&h, pins), [a, b], "Shift-click keeps the first pin");
    click(&mut h, c, Modifiers::NONE);
    assert_eq!(selected(&h, pins), [c], "a plain click selects only the clicked pin");

    click(&mut h, a, Modifiers::NONE);
    click(&mut h, b, Modifiers::SHIFT);
    h.state_mut().session.execute("edit.clear", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(pin_count(&h, layer), 1, "Delete removes both selected pins");
    assert!(h.state().session.active_comp().unwrap().layer(LayerId(layer)).is_some(), "the layer stays");
    h.state_mut().session.undo();
    assert_eq!(pin_count(&h, layer), 3, "one Undo restores both pins");
}
