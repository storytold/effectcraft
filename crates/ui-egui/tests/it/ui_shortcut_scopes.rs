//! Shortcut scopes: one chord can run a different command in a panel than in the rest of the
//! application, and the Timeline's keys work with the Composition panel focused.

use effectcraft_engine::Session;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::{EffectcraftApp, native_menu};
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
    press_in(&egui::Context::default(), app, key, modifiers);
}

fn press_in(ctx: &egui::Context, app: &mut EffectcraftApp, key: Key, modifiers: Modifiers) {
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

fn folders(app: &EffectcraftApp) -> usize {
    app.session.project.items.values().filter(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Folder)).count()
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

#[test]
fn new_folders_chord_opens_a_new_viewer_in_a_viewer_only() {
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
    let viewers = app.ui.viewers.len();
    press(&mut app, Key::N, chord);
    assert_eq!(folders(&app), before, "in the Composition panel the chord is New Viewer");
    assert!(app.ui.viewers.len() > viewers, "and it opens one");
}

#[test]
fn new_viewers_own_keys_work_in_every_panel() {
    for focused in [PanelKind::Project, PanelKind::Timeline, PanelKind::Composition] {
        let mut app = app();
        let viewers = app.ui.viewers.len();
        app.ui.focused = focused;
        press(&mut app, Key::N, Modifiers::ALT | Modifiers::SHIFT);
        assert!(app.ui.viewers.len() > viewers, "{focused:?}");
    }
}

#[test]
fn without_a_comp_new_folders_chord_makes_a_folder_in_a_viewer() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.focused = PanelKind::Composition;
    press(&mut app, Key::N, Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(folders(&app), 1, "{}", app.ui.status);
}

#[test]
fn duplicate_render_items_chord_duplicates_the_selected_item() {
    let chord = Modifiers::COMMAND | Modifiers::SHIFT;
    let mut app = app();
    app.session.execute("renderQueue.add", json!({"output": "/out/second.mov"})).unwrap();
    app.ui.focused = PanelKind::RenderQueue;
    press(&mut app, Key::D, chord);
    assert_eq!(render_items(&app), 2, "nothing is selected");
    assert_eq!(app.ui.status, "select a Render Queue item first");

    let first = app.session.project.render_queue[0].id;
    let ctx = egui::Context::default();
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("rq-ui"), (Vec::<u64>::new(), Some(first))));
    press_in(&ctx, &mut app, Key::D, chord);
    let queue = &app.session.project.render_queue;
    assert_eq!(queue.len(), 3);
    assert_eq!(queue[1].output, queue[0].output, "the copy of the selected item follows it");
}

/// Choose the native menu item that runs `command`, with Cmd held when it is the key equivalent.
fn menu_item(app: &mut EffectcraftApp, command: &str, key_equivalent: bool) {
    let spec = native_menu::build(app);
    let id = spec.items().into_iter().find(|i| i.command == command).unwrap().id.clone();
    let ctx = egui::Context::default();
    let raw =
        egui::RawInput { events: vec![egui::Event::ModifiersChanged(if key_equivalent { Modifiers::COMMAND } else { Modifiers::NONE })], ..Default::default() };
    let mut output = ctx.run_ui(raw, |ui| {
        let _ = native_menu::activate(app, ui.ctx(), &spec, &id);
    });
    output.textures_delta.clear();
}

#[test]
fn the_menu_bars_key_equivalents_follow_the_focused_scope() {
    // New Folder's chord in a viewer.
    let mut app = app();
    let (before, viewers) = (folders(&app), app.ui.viewers.len());
    app.ui.focused = PanelKind::Composition;
    menu_item(&mut app, "project.newFolder", true);
    assert_eq!(folders(&app), before);
    assert!(app.ui.viewers.len() > viewers);
    // Outside one it is the item's own command.
    app.ui.focused = PanelKind::Timeline;
    menu_item(&mut app, "project.newFolder", true);
    assert_eq!(folders(&app), before + 1);

    // Split Layer's chord in the Render Queue.
    let mut app = self::app();
    app.ui.focused = PanelKind::RenderQueue;
    menu_item(&mut app, "edit.splitLayer", true);
    assert_eq!((layers(&app), render_items(&app)), (1, 2));

    // Copy with Property Links' chord in the Project panel.
    let mut app = self::app();
    let clip = app.session.execute("file.importPlaceholder", json!({"name": "Clip"})).unwrap()["item"].as_u64().unwrap();
    app.session.state.project_selection = vec![effectcraft_engine::project::ItemId(clip)];
    let clipboard = app.session.state.clipboard.len();
    app.ui.focused = PanelKind::Project;
    menu_item(&mut app, "edit.copyWithPropertyLinks", true);
    assert!(app.session.state.interpretation.is_some(), "Remember Interpretation ran");
    assert_eq!(app.session.state.clipboard.len(), clipboard);
}

#[test]
fn choosing_a_menu_item_with_the_mouse_runs_that_item() {
    let mut app = app();
    let before = folders(&app);
    app.ui.focused = PanelKind::Composition;
    menu_item(&mut app, "project.newFolder", false);
    assert_eq!(folders(&app), before + 1);
}
