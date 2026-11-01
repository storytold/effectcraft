//! Script Console accessible names and keyboard operation (docs/gaps.md gap 25): the input, the
//! output (a log), Run and Clear are named and reachable with Tab, Tab leaves the input instead of
//! typing a tab character, and Enter / Space work the buttons.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

const INPUT: &str = "Script… (⌘/Ctrl+Enter runs)";

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::ScriptConsole);
    h.state_mut().ui.maximized = Some(PanelKind::ScriptConsole);
    h.run_steps(4);
    h
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.run_steps(3);
}

fn focused(h: &Harness<'_, EffectcraftApp>) -> Option<(Role, String)> {
    h.root().children_recursive().find(|n| n.is_focused()).map(|n| (n.accesskit_node().role(), n.accesskit_node().label().unwrap_or_default()))
}

#[test]
fn input_output_and_buttons_are_named() {
    let mut h = harness();
    assert_eq!(h.get_by_role_and_label(Role::MultilineTextInput, INPUT).accesskit_node().role(), Role::MultilineTextInput);
    let log = h.get_by_role_and_label(Role::Log, "Script Console output");
    assert!(log.accesskit_node().value().is_none_or(|v| v.is_empty()), "an empty console has an empty log");
    // Run is unavailable with nothing typed; Clear with nothing to clear.
    assert!(h.get_by_role_and_label(Role::Button, "Run").accesskit_node().is_disabled());
    assert!(h.get_by_role_and_label(Role::Button, "Clear").accesskit_node().is_disabled());
    h.state_mut().ui.script_console.input = "1 + 1".into();
    h.state_mut().ui.script_console.record("1 + 1", &Ok(json!({"result": 2})));
    h.run_steps(4);
    assert!(!h.get_by_role_and_label(Role::Button, "Run").accesskit_node().is_disabled());
    assert!(!h.get_by_role_and_label(Role::Button, "Clear").accesskit_node().is_disabled());
    let text = h.get_by_role_and_label(Role::Log, "Script Console output").accesskit_node().value().unwrap_or_default();
    assert!(text.contains("> 1 + 1") && text.contains('2'), "the log carries the output lines: {text:?}");
}

#[test]
fn tab_leaves_the_input_and_reaches_run_and_clear() {
    let mut h = harness();
    h.state_mut().ui.script_console.input = "x".into();
    h.state_mut().ui.script_console.record("x", &Ok(json!({"result": 1})));
    h.run_steps(3);
    h.get_by_role_and_label(Role::MultilineTextInput, INPUT).focus();
    h.run_steps(3);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::MultilineTextInput));
    let mut seen = vec![];
    for _ in 0..6 {
        key(&mut h, Key::Tab, Modifiers::NONE);
        if let Some(f) = focused(&h) {
            seen.push(f);
        }
    }
    assert_eq!(h.state().ui.script_console.input, "x", "Tab does not type a tab character into the script");
    for want in ["Run", "Clear"] {
        assert!(seen.iter().any(|(r, n)| *r == Role::Button && n == want), "Tab reaches {want}; reached {seen:?}");
    }
}

#[test]
fn enter_runs_the_script_and_space_clears_the_log_from_the_buttons() {
    let mut h = harness();
    h.state_mut().ui.script_console.input = "app.project.numItems".into();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Run").focus();
    h.run_steps(3);
    assert_eq!(focused(&h), Some((Role::Button, "Run".to_string())));
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().ui.script_console.input.is_empty(), "Run ran the script and cleared the input");
    assert!(h.state().ui.script_console.log.len() >= 2, "its input and result are in the log: {:?}", h.state().ui.script_console.log);
    h.get_by_role_and_label(Role::Button, "Clear").focus();
    h.run_steps(3);
    key(&mut h, Key::Space, Modifiers::NONE);
    assert!(h.state().ui.script_console.log.is_empty(), "Space on Clear empties the log");
}

#[test]
fn ctrl_enter_runs_from_the_input() {
    let mut h = harness();
    h.get_by_role_and_label(Role::MultilineTextInput, INPUT).focus();
    h.run_steps(3);
    h.state_mut().ui.script_console.input = "1 + 2".into();
    h.run_steps(2);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    key(&mut h, Key::Enter, Modifiers::COMMAND);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert!(h.state().ui.script_console.log.iter().any(|l| l.text.contains("> 1 + 2")), "Ctrl+Enter ran it");
}
