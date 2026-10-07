//! CPU-only effect-label picker and panel-display preferences. No render worker starts.
use effectcraft_engine::{Session, color::Label, project::LayerId};
use effectcraft_ui_egui::{
    EffectcraftApp,
    dock::PanelKind,
    panels::{self, effect_controls},
};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Original"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [layer], "effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    s.execute("prefs.set", json!({"key": "labels.0.name", "value": "Priority"})).unwrap();
    s.state.selected_layers = vec![LayerId(layer)];
    s.state.selected_props.clear();
    let mut h = Harness::builder().with_size(vec2(900.0, 600.0)).build_ui_state(
        |ui, app: &mut EffectcraftApp| {
            let installed = egui::Id::new("effect-label-fixture-fonts");
            if !ui.data(|d| d.get_temp::<bool>(installed).unwrap_or(false)) {
                effectcraft_ui_egui::theme::install(ui.ctx(), &app.tokens);
                ui.data_mut(|d| d.insert_temp(installed, true));
                return;
            }
            app.auto.begin_frame();
            effect_controls::show(app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(550.0, 500.0)));
            panels::panel_menu_popup(app, ui);
        },
        EffectcraftApp::new(s),
    );
    h.run_steps(3);
    (h, layer, fx)
}
fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(pos));
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}
fn current(h: &Harness<'_, EffectcraftApp>, layer: u64, fx: u64) -> Label {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().effect_label
}

#[test]
fn effect_label_picker_changes_only_instance_label_and_can_undo() {
    let (mut h, layer, fx) = harness();
    let e = h.state().auto.find(&format!("effectControls.effect.{fx}.label")).unwrap();
    let pos = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    click(&mut h, pos);
    h.get_by_label("Priority").click();
    h.run_steps(3);
    assert_eq!(current(&h, layer, fx), Label::Red);
    assert!(h.state().session.state.selected_props.is_empty(), "swatch must not steal effect title selection");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(current(&h, layer, fx), Label::None);
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(current(&h, layer, fx), Label::Red);
}

#[test]
fn effect_label_panel_display_toggles_preserve_authored_label() {
    let (mut h, layer, fx) = harness();
    h.state_mut().session.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": "Blue"})).unwrap();
    for name in ["Show Effect Label Swatch", "Use Label Color for Effect Background"] {
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new("panel-menu"), (PanelKind::EffectControls, pos2(580.0, 50.0))));
        h.run_steps(3);
        h.get_by_label(name).click();
        h.run_steps(3);
        assert_eq!(current(&h, layer, fx), Label::Blue);
    }
    assert!(!h.state().session.prefs.appearance.show_effect_label_swatch);
    assert!(!h.state().session.prefs.appearance.use_label_color_for_effect_background);
    assert!(h.state().auto.find(&format!("effectControls.effect.{fx}.label")).is_none());
}
