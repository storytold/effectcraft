//! Shortcut scopes: one chord can run a different command in a panel than in the rest of the
//! application, and the Timeline's keys work with the Composition panel focused.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Key, Modifiers};
use serde_json::json;

/// A comp with one solid selected at 1 s, and one render item.
fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 160, "height": 90, "frameRate": 30, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#406080"})).unwrap();
    s.execute("renderQueue.add", json!({"output": "/out/main.mov"})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    EffectcraftApp::new(s)
}

/// Press `key` with `modifiers` and run the shortcut dispatcher on it.
fn press(app: &mut EffectcraftApp, key: Key, modifiers: Modifiers) {
    let ctx = egui::Context::default();
    let raw = egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], ..Default::default() };
    let mut output = ctx.run_ui(raw, |ui| effectcraft_ui_egui::menus::handle_shortcuts(app, ui.ctx()));
    output.textures_delta.clear();
}

fn layers(app: &EffectcraftApp) -> usize {
    app.session.active_comp().map_or(0, |c| c.layers.len())
}

fn render_items(app: &EffectcraftApp) -> usize {
    app.session.project.render_queue.len()
}

#[test]
fn a_panels_shortcut_wins_in_its_panel_and_the_applications_everywhere_else() {
    let mut app = app();
    // Duplicate Render Item (Render Queue) on Split Layer's chord.
    app.session.execute("shortcuts.set", json!({"command": "renderQueue.duplicate", "keys": "Cmd+Shift+D"})).unwrap();
    let chord = Modifiers::COMMAND | Modifiers::SHIFT;

    app.ui.focused = PanelKind::Timeline;
    press(&mut app, Key::D, chord);
    assert_eq!((layers(&app), render_items(&app)), (2, 1), "in the Timeline the chord splits the layer");

    app.ui.focused = PanelKind::RenderQueue;
    press(&mut app, Key::D, chord);
    assert_eq!((layers(&app), render_items(&app)), (2, 2), "in the Render Queue it duplicates the render item");

    app.ui.focused = PanelKind::Project;
    app.session.execute("edit.selectAll", json!({})).unwrap();
    app.session.execute("time.set", json!({"time": 2.0})).unwrap();
    press(&mut app, Key::D, chord);
    assert_eq!((layers(&app), render_items(&app)), (3, 2), "and in any other panel it is Split Layer again");
}

#[test]
fn a_panels_shortcut_that_nothing_else_claims_runs_from_any_panel() {
    let mut app = app();
    let unclaimed = "Cmd+Alt+Shift+F12";
    assert!(app.session.shortcuts().bindings().iter().all(|(sc, _)| *sc != unclaimed), "{unclaimed} has a default now; pick a free chord");
    app.session.execute("shortcuts.set", json!({"command": "renderQueue.duplicate", "keys": unclaimed})).unwrap();
    app.ui.focused = PanelKind::Timeline;
    press(&mut app, Key::F12, Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(render_items(&app), 2);
}

#[test]
fn the_timelines_keys_work_with_the_composition_panel_focused() {
    for focused in [PanelKind::Timeline, PanelKind::Composition] {
        let mut app = app();
        app.ui.focused = focused;
        // B: the work area begins at the current time (a Timeline shortcut).
        press(&mut app, Key::B, Modifiers::NONE);
        let start = app.session.active_comp().map(|c| c.work_area.0.seconds());
        assert_eq!(start, Some(1.0), "{focused:?}");
    }
}

#[test]
fn the_default_preset_has_no_two_commands_on_one_chord_in_one_scope() {
    let app = app();
    let c = app.session.shortcuts().all_conflicts();
    assert!(c.is_empty(), "{c:?}");
}
