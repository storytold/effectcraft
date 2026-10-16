//! The Effect Console (egui_kittest): Ctrl+Space opens a search bar at the pointer; typing finds
//! effects, Enter applies the selected one to the selected layers and closes, Shift+Enter keeps
//! it open for another, Escape closes without applying.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemId, LayerId};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// The platform's Ctrl as egui reports it (off macOS Ctrl is also the command key).
fn ctrl() -> Modifiers {
    Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), ..Default::default() }
}

fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    let main = s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Red", "color": [1.0, 0.0, 0.0, 1.0]})).unwrap()["layer"].as_u64().unwrap();
    s.state.open_comps = vec![ItemId(main)];
    s.state.active_comp = Some(ItemId(main));
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, l)
}

fn effects(h: &Harness<'_, EffectcraftApp>, l: u64) -> Vec<String> {
    let s = &h.state().session;
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap();
    layer.props.group("effects").map(|g| g.groups().map(|e| e.match_id.clone()).collect()).unwrap_or_default()
}

fn open(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).any(|e| e.id == "fxConsole.search")
}

fn type_text(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    h.event(Event::Text(text.into()));
    h.run_steps(2);
}

#[test]
fn ctrl_space_opens_at_the_pointer_and_enter_applies_the_best_match() {
    let (mut h, l) = harness();
    h.event(Event::PointerMoved(pos2(700.0, 400.0)));
    h.step();
    h.key_press_modifiers(ctrl(), Key::Space);
    h.run_steps(2);
    assert!(open(&h), "Ctrl+Space opens the console");
    let field = h.state().auto.elements.iter().find(|e| e.id == "fxConsole.search").cloned().unwrap();
    assert!((field.rect[0] + field.rect[2] / 2.0 - 700.0).abs() < 2.0, "centred on the pointer: {:?}", field.rect);

    type_text(&mut h, "gaussian blur");
    let first = h.state().auto.elements.iter().find(|e| e.id == "fxConsole.result.0").map(|e| e.label.clone());
    assert_eq!(first.as_deref(), Some("Gaussian Blur"));
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(effects(&h, l), vec!["ec.blur.gaussian"]);
    assert!(!open(&h), "Enter closes it");
    assert_eq!(h.state().ui.effects_recent.first().map(String::as_str), Some("ec.blur.gaussian"), "remembered as recently used");
}

#[test]
fn shift_enter_keeps_it_open_arrows_choose_and_escape_closes_without_applying() {
    let (mut h, l) = harness();
    h.key_press_modifiers(ctrl(), Key::Space);
    h.run_steps(2);
    type_text(&mut h, "gb");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run_steps(2);
    assert_eq!(effects(&h, l).len(), 1);
    assert!(open(&h), "Shift+Enter keeps the console open");

    // Pick the second result with ↓.
    type_text(&mut h, "blur");
    let second = h.state().auto.elements.iter().find(|e| e.id == "fxConsole.result.1").map(|e| e.label.clone()).unwrap();
    h.key_press(Key::ArrowDown);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(2);
    let fx = effects(&h, l);
    assert_eq!(fx.len(), 2);
    let name = effectcraft_engine::effects::find(&fx[1]).map(|e| e.name).unwrap();
    assert_eq!(name, second);

    // Escape closes and applies nothing.
    h.key_press_modifiers(ctrl(), Key::Space);
    h.run_steps(2);
    type_text(&mut h, "glow");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!open(&h));
    assert_eq!(effects(&h, l).len(), 2);
}

#[test]
fn without_a_selected_layer_nothing_is_applied_and_the_status_says_why() {
    let (mut h, l) = harness();
    h.state_mut().session.state.selected_layers.clear();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.fxConsole", json!({"query": "glow"})).unwrap();
    h.run_steps(2);
    assert!(open(&h), "app.fxConsole opens it (control channel / MCP)");
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert!(effects(&h, l).is_empty());
    assert!(h.state().ui.status.contains("Select a layer"), "{}", h.state().ui.status);
}
