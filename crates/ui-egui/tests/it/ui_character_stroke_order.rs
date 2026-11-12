//! The Character panel's stroke order menu: the two per-character orders and the two layer-wide
//! ones (All Fills Over All Strokes, All Strokes Over All Fills), which set the text layer's
//! More Options ▸ Fill & Stroke.
use effectcraft_engine::{Session, commands::text_edit::layer_doc, project::LayerId};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{
    Event, Modifiers, PointerButton, Pos2,
    accesskit::{Role, Toggled},
    pos2, vec2,
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn open_menu(h: &mut Harness<'_, EffectcraftApp>) {
    let e = h.state().auto.find("character.strokeOverFill").unwrap_or_else(|| panic!("missing character.strokeOverFill")).clone();
    click(h, pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5));
}

fn pick(h: &mut Harness<'_, EffectcraftApp>, label: &str) {
    open_menu(h);
    // A menu row: the dropdown shows the current order under the same label.
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

fn fill_stroke(s: &mut Session, layer: u64) -> serde_json::Value {
    s.execute("prop.get", json!({"layer": layer, "path": "text/moreOptions/fillStroke"})).unwrap()["value"].clone()
}

#[test]
fn character_stroke_order_menu_sets_layer_fill_and_stroke() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 2})).unwrap();
    let text = s.execute("layer.newText", json!({"text": "Caption", "font": "Inter"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setText", json!({"layer": text, "applyStroke": true, "strokeWidth": 8.0, "strokeOverFill": false})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.set_workspace("Text");
    let builder = Harness::builder().with_size(vec2(1600.0, 1400.0));
    let mut h = if std::env::var_os("EC_SNAPSHOT_DIR").is_some() { builder.wgpu().build_eframe(|_| app) } else { builder.build_eframe(|_| app) };
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(4);

    pick(&mut h, "All Fills Over All Strokes");
    assert_eq!(fill_stroke(&mut h.state_mut().session, text), json!(1));
    pick(&mut h, "All Strokes Over All Fills");
    assert_eq!(fill_stroke(&mut h.state_mut().session, text), json!(2));
    // The menu shows the layer's order as the current one.
    open_menu(&mut h);
    assert_eq!(h.get_by_role_and_label(Role::Button, "All Strokes Over All Fills").accesskit_node().toggled(), Some(Toggled::True));
    assert_eq!(h.get_by_role_and_label(Role::Button, "Fill Over Stroke").accesskit_node().toggled(), Some(Toggled::False));
    if let Ok(dir) = std::env::var("EC_SNAPSHOT_DIR") {
        h.render().unwrap().save(format!("{dir}/character-stroke-order.png")).unwrap();
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // A per-character order goes back to the Per Character Palette, in one undo step.
    pick(&mut h, "Stroke Over Fill");
    assert_eq!(fill_stroke(&mut h.state_mut().session, text), json!(0));
    assert!(layer_doc(&h.state().session, LayerId(text)).unwrap().stroke_over_fill);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(fill_stroke(&mut h.state_mut().session, text), json!(2));
    assert!(!layer_doc(&h.state().session, LayerId(text)).unwrap().stroke_over_fill);
}
