//! A plug-in's gradient parameter in Effect Controls: its strip opens the Gradient Editor on it.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParam, PluginParamKind, PluginParams, register_plugin};
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Modifiers, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

struct Ramp {
    m: PluginManifest,
}

impl EffectPlugin for Ramp {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn gradient_parameter_strip_opens_the_gradient_editor() {
    let m = PluginManifest {
        api: 2,
        id: "org.test.uiramp".into(),
        name: "UI Ramp".into(),
        category: "Generate".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params: vec![PluginParam {
            id: "ramp".into(),
            name: "Ramp".into(),
            kind: PluginParamKind::Gradient { default: Some(json!(["#ff0000", "#0000ff"])) },
            group: String::new(),
        }],
    };
    register_plugin(Arc::new(Ramp { m })).unwrap();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Ramp", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#40b080"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layer": layer, "effect": "org.test.uiramp"})).unwrap()["effects"][0].as_u64().unwrap();
    let uid = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().get("ramp").unwrap().uid;
    s.state.selected_layers = vec![LayerId(layer)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.toggle_maximize(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    assert_ne!(h.state().dialog, Some(Dialog::GradientEditor));
    let [x, y, w, ht] = h.state().auto.find(&format!("effectControls.prop.{uid}.value")).expect("the Ramp strip").rect;
    let p = pos2(x + w / 2.0, y + ht / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
    let d = h.state().gradient_draft();
    assert_eq!(d.target.as_ref().map(|t| (t.layer, t.prop)), Some((layer, uid)));
    assert_eq!(d.colors.len(), 2);
    assert!(d.colors[0].value[0] > 0.99 && d.colors[1].value[2] > 0.99, "{:?}", d.colors);
}
