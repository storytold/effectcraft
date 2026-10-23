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
    app.session.execute("shortcuts.set", json!({"command": "renderQueue.duplicate", "keys": "Cmd+Alt+Shift+D"})).unwrap();
    app.ui.focused = PanelKind::Timeline;
    press(&mut app, Key::D, Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT);
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

#[test]
fn new_folders_chord_opens_a_new_viewer_in_a_viewer_only() {
    let folders = |app: &EffectcraftApp| app.session.project.items.values().filter(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Folder)).count();
    let chord = Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT;
    // The Timeline shares the Composition panel's shortcuts, but it isn't a viewer.
    for focused in [PanelKind::Project, PanelKind::Timeline] {
        let mut app = app();
        let before = folders(&app);
        app.ui.focused = focused;
        press(&mut app, Key::N, chord);
        assert_eq!(folders(&app), before + 1, "{focused:?}");
    }
    let mut app = app();
    let before = folders(&app);
    app.ui.focused = PanelKind::Composition;
    press(&mut app, Key::N, chord);
    assert_eq!(folders(&app), before, "in the Composition panel the chord is New Viewer");
}

/// Add Marker has a main-keyboard key on macOS (Ctrl+8) for keyboards without a numeric keypad;
/// like numpad *, it marks the selected layers, or the composition when none is selected.
#[cfg(target_os = "macos")]
#[test]
fn ctrl_8_adds_a_marker_like_numpad_multiply() {
    let mut app = app();
    let layer_markers = |app: &EffectcraftApp| app.session.active_comp().map_or(0, |c| c.layers.iter().map(|l| l.markers.len()).sum::<usize>());
    let comp_markers = |app: &EffectcraftApp| app.session.active_comp().map_or(0, |c| c.markers.len());
    app.session.execute("edit.selectAll", json!({})).unwrap();
    press(&mut app, Key::Num8, Modifiers::CTRL);
    assert_eq!((layer_markers(&app), comp_markers(&app)), (1, 0));
    app.session.execute("edit.deselectAll", json!({})).unwrap();
    press(&mut app, Key::Num8, Modifiers::CTRL);
    assert_eq!((layer_markers(&app), comp_markers(&app)), (1, 1));
}

/// With a paint tool X swaps the paint colours; otherwise it is the Timeline's key.
#[test]
fn paint_keys_work_only_with_a_paint_tool() {
    let mut app = app();
    app.ui.focused = PanelKind::Composition;
    app.session.execute("paint.options", json!({"color": [1, 0, 0, 1], "background": [0, 0, 1, 1]})).unwrap();
    press(&mut app, Key::X, Modifiers::NONE);
    assert_eq!(app.session.state.paint.color, [1.0, 0.0, 0.0, 1.0], "the Selection tool: not a paint key");
    app.ui.tool = effectcraft_ui_egui::state::Tool::Brush;
    press(&mut app, Key::X, Modifiers::NONE);
    assert_eq!(app.session.state.paint.color, [0.0, 0.0, 1.0, 1.0]);
    press(&mut app, Key::D, Modifiers::NONE);
    assert_eq!(app.session.state.paint.color, [0.0, 0.0, 0.0, 1.0]);
}

/// With a mask selected its mode has single-letter keys; with one of its properties selected
/// (or nothing) the letters stay the Timeline's reveal keys.
#[test]
fn mask_mode_keys_work_only_with_a_mask_selected() {
    let mut app = app();
    app.ui.focused = PanelKind::Timeline;
    app.session.execute("edit.selectAll", json!({})).unwrap();
    app.session.execute("layer.addMask", json!({})).unwrap();
    let mode = |app: &EffectcraftApp| {
        let c = app.session.active_comp().unwrap();
        let l = c.layers.iter().find(|l| l.masks().is_some_and(|m| m.groups().next().is_some())).unwrap();
        format!("{:?}", l.masks().unwrap().groups().next().unwrap().kind)
    };
    let added = mode(&app);
    app.session.state.selected_props.clear();
    press(&mut app, Key::S, Modifiers::NONE);
    assert_eq!(mode(&app), added, "no mask selected: S reveals Scale");
    app.session.execute("layer.mask.selectNext", json!({})).unwrap();
    press(&mut app, Key::S, Modifiers::NONE);
    assert!(mode(&app).contains("Subtract"), "{}", mode(&app));
    press(&mut app, Key::I, Modifiers::NONE);
    assert!(mode(&app).contains("Intersect"), "{}", mode(&app));
}

/// Press `key` at `time` seconds in `ctx` (double presses need the same context).
fn press_at(ctx: &egui::Context, app: &mut EffectcraftApp, key: Key, time: f64) {
    let raw = egui::RawInput {
        time: Some(time),
        // (Released too: a key still down would make the next press a repeat.)
        events: [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }).to_vec(),
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| effectcraft_ui_egui::menus::handle_shortcuts(app, ui.ctx()));
    output.textures_delta.clear();
}

/// A double press runs the command bound to it in the shortcut preset, whatever that is.
#[test]
fn a_double_press_runs_its_own_binding() {
    let mut app = app();
    app.ui.focused = PanelKind::Timeline;
    app.session.execute("edit.selectAll", json!({})).unwrap();
    let ctx = egui::Context::default();
    // P, then P again at once: PP (paint strokes, Roto Brush, Puppet) replaces Position.
    press_at(&ctx, &mut app, Key::P, 1.0);
    assert_eq!(app.ui.timeline.reveal, ["position"]);
    press_at(&ctx, &mut app, Key::P, 1.2);
    assert_eq!(app.ui.timeline.reveal, ["paint"], "status: {}", app.ui.status);
    // Later than a double press, P is Position again.
    press_at(&ctx, &mut app, Key::P, 5.0);
    assert_eq!(app.ui.timeline.reveal, ["position"]);

    // Any command can have a double press…
    app.session.execute("shortcuts.set", json!({"command": "layer.newNull", "keys": "F7 F7"})).unwrap();
    let before = layers(&app);
    press_at(&ctx, &mut app, Key::F7, 10.0);
    assert_eq!(layers(&app), before, "one press isn't the double");
    press_at(&ctx, &mut app, Key::F7, 10.3);
    assert_eq!(layers(&app), before + 1);
    // …and without one bound, the second press is the chord's own command again.
    app.session.execute("edit.selectAll", json!({})).unwrap();
    app.session.execute("shortcuts.set", json!({"command": "timeline.reveal.paint", "keys": []})).unwrap();
    press_at(&ctx, &mut app, Key::P, 20.0);
    assert!(app.ui.timeline.reveal.is_empty(), "Position was showing: P hides it");
    press_at(&ctx, &mut app, Key::P, 20.2);
    assert_eq!(app.ui.timeline.reveal, ["position"], "P again, not PP");
}

/// The Show Snapshot keys show while held: pressing one doesn't switch Show Snapshot on.
#[test]
fn a_show_snapshot_key_does_not_latch() {
    let mut app = app();
    app.session.execute("view.takeSnapshot", json!({"scale": 0.25})).unwrap();
    app.session.execute("view.takeSnapshot2", json!({"scale": 0.25})).unwrap();
    for key in [Key::F5, Key::F6] {
        press(&mut app, key, Modifiers::NONE);
        assert!(!app.session.state.viewer.show_snapshot, "{key:?}");
    }
    // Shift+F7 takes slot 3; Cmd+Shift+F6 purges slot 2.
    press(&mut app, Key::F7, Modifiers::SHIFT);
    press(&mut app, Key::F6, Modifiers::COMMAND | Modifiers::SHIFT);
    let filled: Vec<bool> = app.session.snapshot_slots.iter().map(Option::is_some).collect();
    assert_eq!(filled, [true, false, true, false]);
}

/// Cmd+` in the Effect Controls panel twirls the layer's effects; in the Timeline, the layer.
#[test]
fn effect_controls_twirls_its_effects() {
    let mut app = app();
    app.session.execute("edit.selectAll", json!({})).unwrap();
    app.session.execute("effect.apply", json!({"effect": "Tint"})).unwrap();
    let fx = app.session.active_comp().unwrap().layers[0].effects().unwrap().groups().next().unwrap().uid;
    app.ui.focused = PanelKind::EffectControls;
    press(&mut app, Key::Backtick, Modifiers::COMMAND);
    assert!(app.ui.fx_closed.contains(&fx), "open effects close");
    press(&mut app, Key::Backtick, Modifiers::COMMAND);
    assert!(!app.ui.fx_closed.contains(&fx));
    app.ui.focused = PanelKind::Timeline;
    press(&mut app, Key::Backtick, Modifiers::COMMAND);
    assert!(!app.ui.fx_closed.contains(&fx), "the Timeline's Cmd+` leaves Effect Controls alone");
}

/// 1–9 go to the numbered composition markers; with a camera tool 1, 2 and 3 pick the tool.
#[test]
fn digits_go_to_markers_unless_a_camera_tool_is_active() {
    use effectcraft_ui_egui::state::Tool;
    let mut app = app();
    app.session.execute("edit.deselectAll", json!({})).unwrap();
    app.session.execute("time.set", json!({"time": 3.0})).unwrap();
    press(&mut app, Key::Num2, Modifiers::SHIFT);
    let marks: Vec<String> = app.session.active_comp().unwrap().markers.iter().map(|m| m.comment.clone()).collect();
    assert_eq!(marks, ["2"], "Shift+2 sets marker 2");
    app.session.execute("time.set", json!({"time": 0.0})).unwrap();
    press(&mut app, Key::Num2, Modifiers::NONE);
    assert_eq!(app.session.time().seconds(), 3.0);
    assert_eq!(app.ui.tool, Tool::Selection);
    // A camera tool: 2 is the Pan Camera tool, and the time stays.
    app.ui.tool = Tool::Orbit;
    app.session.execute("time.set", json!({"time": 0.0})).unwrap();
    press(&mut app, Key::Num2, Modifiers::NONE);
    assert_eq!((app.ui.tool, app.session.time().seconds()), (Tool::PanCamera, 0.0));
}

#[test]
fn alt_apostrophe_shows_the_proportional_grid() {
    let mut app = app();
    app.ui.focused = PanelKind::Composition;
    press(&mut app, Key::Quote, Modifiers::ALT);
    assert!(app.ui.viewer.proportional_grid);
    press(&mut app, Key::Quote, Modifiers::ALT);
    assert!(!app.ui.viewer.proportional_grid);
}

/// One key event at `time` seconds in `ctx`.
fn key_event(ctx: &egui::Context, app: &mut EffectcraftApp, key: Key, pressed: bool, time: f64) {
    let raw = egui::RawInput {
        time: Some(time),
        events: vec![egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }],
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| effectcraft_ui_egui::menus::handle_shortcuts(app, ui.ctx()));
    output.textures_delta.clear();
}

/// A tool's key held down gives the tool until it is released; a tap keeps the tool.
#[test]
fn a_held_tool_key_is_the_tool_for_a_moment() {
    use effectcraft_ui_egui::state::Tool;
    let mut app = app();
    let ctx = egui::Context::default();
    key_event(&ctx, &mut app, Key::H, true, 1.0);
    assert_eq!(app.ui.tool, Tool::Hand);
    key_event(&ctx, &mut app, Key::H, false, 2.0);
    assert_eq!(app.ui.tool, Tool::Selection, "held: back to the tool from before");
    key_event(&ctx, &mut app, Key::H, true, 3.0);
    key_event(&ctx, &mut app, Key::H, false, 3.1);
    assert_eq!(app.ui.tool, Tool::Hand, "tapped: the tool stays");
    // The key of the tool that is already active changes nothing on release.
    key_event(&ctx, &mut app, Key::H, true, 5.0);
    key_event(&ctx, &mut app, Key::H, false, 6.0);
    assert_eq!(app.ui.tool, Tool::Hand);
}
