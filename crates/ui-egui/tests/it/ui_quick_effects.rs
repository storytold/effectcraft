//! Quick Effects Search: the configurable chord opens one popup, search finds real effects,
//! Enter applies them to the validated selection and undo/redo works.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness(layer: bool) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 160, "height": 90, "duration": 4})).unwrap();
    if layer {
        s.execute("layer.newSolid", json!({"name": "Blue", "color": "#406080"})).unwrap();
    }
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_context_setup(effectcraft_ui_egui::framework::install_headless).build_eframe(|_| {
        let mut app = EffectcraftApp::new(s);
        app.use_manual_clock();
        app
    })
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.run_steps(3);
}

/// Enter resolves against the query after TextEdit processed this frame's text events.
#[test]
fn enter_after_a_text_edit_uses_the_edited_query() {
    let mut h = harness(true);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "Gaussian Blur".into();
    h.run_steps(2);
    assert_eq!(effect_count(&h), 0);
    for ch in "zzzz".chars() {
        h.input_mut().events.push(Event::Text(ch.to_string()));
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().ui.quick_effects.query.contains("zzzz"), "the edit reached the field");
    assert_eq!(effect_count(&h), 0, "Enter must not apply a result from the query before the edit");
}

/// A text preset is applied to the selected text layer, not the first selected solid.
#[test]
fn text_preset_targets_a_text_layer_in_the_selection() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 160, "height": 90, "duration": 4})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap();
    let solid_id = solid["layer"].as_u64().unwrap();
    let text = s.execute("layer.newText", json!({"text": "Hello", "name": "Text"})).unwrap();
    let text_id = text["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [solid_id, text_id]})).unwrap();
    let mut h =
        Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_context_setup(effectcraft_ui_egui::framework::install_headless).build_eframe(|_| {
            let mut app = EffectcraftApp::new(s);
            app.use_manual_clock();
            app
        });
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "fade up".into();
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    let comp = h.state().session.active_comp().unwrap();
    let text_layers = comp.layer(effectcraft_engine::project::LayerId(text_id)).unwrap().props.group("text/animators").map(|a| a.groups().count()).unwrap_or(0);
    assert!(text_layers > 0, "Fade Up Characters was applied to the text layer");
    assert!(comp.layer(effectcraft_engine::project::LayerId(solid_id)).unwrap().props.group("text/animators").is_none(), "the solid was not targeted");
}

/// Animation presets shown in Effects & Presets are searchable too, not just registered effects.
#[test]
fn animation_presets_are_searchable() {
    let mut h = harness(true);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "fade up".into();
    h.run_steps(2);
    assert!(rows(&h).iter().any(|n| n == "Fade Up Characters"), "`fade up` finds the Fade Up Characters preset: {:?}", rows(&h));
}

/// Type the way a real keyboard does: a key press, a text event and a release per character.
fn type_chars(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    for ch in text.chars() {
        let key = match ch.to_ascii_lowercase() {
            'a' => Key::A,
            'b' => Key::B,
            'c' => Key::C,
            'd' => Key::D,
            'e' => Key::E,
            'f' => Key::F,
            'g' => Key::G,
            'h' => Key::H,
            'i' => Key::I,
            'j' => Key::J,
            'k' => Key::K,
            'l' => Key::L,
            'm' => Key::M,
            'n' => Key::N,
            'o' => Key::O,
            'p' => Key::P,
            'q' => Key::Q,
            'r' => Key::R,
            's' => Key::S,
            't' => Key::T,
            'u' => Key::U,
            'v' => Key::V,
            'w' => Key::W,
            'x' => Key::X,
            'y' => Key::Y,
            'z' => Key::Z,
            _ => continue,
        };
        for pressed in [true, false] {
            h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        }
        h.input_mut().events.push(Event::Text(ch.to_string()));
        h.step();
    }
    h.run_steps(3);
}

fn rows(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().auto.elements.iter().filter(|e| e.id.starts_with("quickEffects.item.")).map(|e| e.label.clone()).collect()
}

fn open(h: &mut Harness<'_, EffectcraftApp>) {
    key(h, Key::Space, Modifiers::SHIFT);
    assert!(h.state().ui.quick_effects.open, "Shift+Space opened Quick Effects Search");
}

fn effect_count(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.active_comp().and_then(|c| c.layers.first()).and_then(|l| l.effects()).map_or(0, |fx| fx.groups().count())
}

/// The app's own UI command list, as the shortcut table is built from it.
fn ui_commands() -> Vec<effectcraft_engine::shortcuts::UiCommand> {
    effectcraft_ui_egui::menus::UI_COMMANDS
        .iter()
        .map(|c| effectcraft_engine::shortcuts::UiCommand { id: c.id.into(), label: c.label.into(), shortcut: c.shortcut.map(str::to_string), scope: None })
        .collect()
}

#[test]
fn shift_space_opens_one_popup_and_a_custom_chord_replaces_it() {
    let mut h = harness(true);
    open(&mut h);
    open(&mut h);
    assert!(h.state().ui.quick_effects.open, "reopening keeps a single popup");

    h.state_mut().session.execute("shortcuts.set", json!({"command": "effects.quickSearch", "keys": "Cmd+Alt+Shift+K"})).unwrap();
    h.state_mut().ui.quick_effects.open = false;
    key(&mut h, Key::Space, Modifiers::SHIFT);
    assert!(!h.state().ui.quick_effects.open, "the old chord no longer opens it");
    key(&mut h, Key::K, Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT);
    assert!(h.state().ui.quick_effects.open, "the reassigned chord opens it");
    assert_eq!(h.state().session.shortcuts().shortcut_of("effects.quickSearch", &json!(null)), Some("Cmd+Alt+Shift+K"));

    // The override survives the existing preset export/import round trip (the editor's own path).
    let exported = h.state().session.keymaps.export(None).unwrap();
    assert_eq!(exported["overrides"]["effects.quickSearch"], json!(["Cmd+Alt+Shift+K"]), "the custom chord is exported");
    let mut reloaded = effectcraft_engine::shortcuts::Keymaps::default();
    reloaded.import(&exported, Some("Round Trip")).unwrap();
    let table = effectcraft_engine::shortcuts::ShortcutTable::build(&reloaded, &ui_commands());
    assert_eq!(table.keys.get("effects.quickSearch").map(Vec::as_slice), Some(["Cmd+Alt+Shift+K".to_string()].as_slice()), "import keeps the custom chord");
    h.state_mut().session.execute("shortcuts.set", json!({"command": "effects.quickSearch", "keys": []})).unwrap();
    assert_eq!(h.state().session.shortcuts().shortcut_of("effects.quickSearch", &json!(null)), None, "clearing works");
    h.state_mut().session.execute("shortcuts.reset", json!({"command": "effects.quickSearch"})).unwrap();
    assert_eq!(h.state().session.shortcuts().shortcut_of("effects.quickSearch", &json!(null)), Some("Shift+Space"), "reset restores the default");
}

#[test]
fn typing_a_real_effect_name_shows_real_results() {
    let mut h = harness(true);
    open(&mut h);
    // Type as a keyboard does: key press, text, release for each character.
    type_chars(&mut h, "blur");
    assert_eq!(h.state().ui.quick_effects.query, "blur", "the query follows the keystrokes");
    let found = rows(&h);
    assert!(!found.is_empty(), "typing `blur` renders real effect rows, got none");
    assert!(found.iter().any(|n| n.to_lowercase().contains("blur")), "the rows are real blur effects: {found:?}");

    // Changing the query changes the results; clearing it removes every row.
    for q in ["drop", "shadow", "color", "glow", "transform", "audio"] {
        h.state_mut().ui.quick_effects.query.clear();
        type_chars(&mut h, q);
        assert!(!rows(&h).is_empty(), "`{q}` must render real effects");
    }
    h.state_mut().ui.quick_effects.query.clear();
    h.run_steps(2);
    assert!(rows(&h).is_empty(), "clearing the query removes all rows");
}

#[test]
fn typed_search_and_enter_apply_a_real_effect() {
    let mut h = harness(true);
    open(&mut h);
    type_chars(&mut h, "gaussian");
    assert!(!rows(&h).is_empty(), "a real Gaussian effect is listed");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(effect_count(&h), 1, "Enter applied the highlighted real effect to the layer");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(effect_count(&h), 0, "the apply is undoable");
}

#[test]
fn the_popup_can_be_moved_and_reopens_where_it_was_left() {
    let mut h = harness(true);
    open(&mut h);
    h.run_steps(4);
    let before = h.state().auto.find("quickEffects.search").expect("field").rect;
    // Drag from the frame padding above the field (the field itself is for typing).
    let start = pos2(before[0] + 200.0, before[1] - 6.0);
    h.input_mut().events.push(Event::PointerMoved(start));
    h.input_mut().events.push(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let end = pos2(start.x + 90.0, start.y + 70.0);
    h.input_mut().events.push(Event::PointerMoved(end));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.run_steps(4);
    let moved = h.state().auto.find("quickEffects.search").expect("field").rect;
    assert!(moved[0] > before[0] + 50.0 && moved[1] > before[1] + 40.0, "the popup follows the drag: {before:?} -> {moved:?}");

    // Close and reopen: it must come back at the moved position, not the window centre.
    h.state_mut().ui.quick_effects.open = false;
    h.run_steps(2);
    open(&mut h);
    h.run_steps(4);
    let reopened = h.state().auto.find("quickEffects.search").expect("field").rect;
    assert_eq!((reopened[0], reopened[1]), (moved[0], moved[1]), "it reopens where it was last moved");
}

#[test]
fn opening_leaves_an_empty_field_with_no_clear_icon_or_gap() {
    let mut h = harness(true);
    open(&mut h);
    h.run_steps(4);
    assert_eq!(h.state().ui.quick_effects.query, "", "the opening chord must not insert a space");
    assert!(h.state().auto.find("quickEffects.clear").is_none(), "no clear icon when the field is empty");
    assert!(h.state().auto.find("quickEffects.list").is_none(), "no results box below the field while empty");
}

#[test]
fn the_moved_position_is_saved_in_global_prefs_for_the_next_project() {
    let mut h = harness(true);
    open(&mut h);
    h.run_steps(4);
    let field = h.state().auto.find("quickEffects.search").expect("field").rect;
    let start = pos2(field[0] + 200.0, field[1] - 6.0);
    h.input_mut().events.push(Event::PointerMoved(start));
    h.input_mut().events.push(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let end = pos2(start.x + 90.0, start.y + 70.0);
    h.input_mut().events.push(Event::PointerMoved(end));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    let saved = h.state().session.prefs.quick_effects_pos;
    assert!(saved.is_some(), "the dragged position is written to global preferences");

    // A new project starts with a fresh UiState but the same preferences.
    let mut fresh = harness(true);
    fresh.state_mut().session.prefs.quick_effects_pos = saved;
    open(&mut fresh);
    fresh.run_steps(3);
    assert_eq!(fresh.state().ui.quick_effects.pos, saved, "the next project opens at the saved position");
}

#[test]
fn an_empty_search_lists_no_effects_at_all() {
    let mut h = harness(true);
    // Seed both favorites and recents: the palette must still show nothing before typing.
    h.state_mut().ui.effects_recent = vec!["ec.blur.gaussian".to_string()];
    open(&mut h);
    h.run_steps(2);
    assert!(h.state().ui.quick_effects.query.is_empty(), "the field starts empty");
    assert_eq!(h.state().ui.quick_effects.sel, 0);
    // Enter with no query applies nothing, even though the layer and recents exist.
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(effect_count(&h), 0, "no effect is listed or applied before typing");
}

#[test]
fn no_selected_layer_does_not_apply_an_effect() {
    let mut h = harness(false);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "Gaussian Blur".into();
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(effect_count(&h), 0, "nothing is applied without a layer");
    // The popup stays open: Enter was not an apply action.
    assert!(h.state().ui.quick_effects.open);
}

#[test]
fn enter_applies_the_matched_effect_and_undo_redo_restore_it() {
    let mut h = harness(true);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "Gaussian Blur".into();
    h.run_steps(2);
    assert_eq!(effect_count(&h), 0);
    let revision = h.state().session.revision;
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(effect_count(&h), 1, "Enter applied the first matching effect");
    assert!(h.state().session.revision > revision, "the project revision bumped for the preview/render refresh");
    let layers = h.state().session.state.selected_layers.clone();
    assert_eq!(layers.len(), 1, "the selection is preserved");

    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(effect_count(&h), 0, "undo removes the effect");
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(effect_count(&h), 1, "redo restores it");
}

#[test]
fn an_unmatched_search_applies_nothing_and_records_nothing() {
    let mut h = harness(true);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "zzzznotaneffect".into();
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(effect_count(&h), 0);
    assert!(h.state().ui.effects_recent.is_empty(), "a failed apply is not recorded as recent");
}

#[test]
fn the_popup_is_compact_and_keeps_five_rows_visible() {
    let mut h = harness(true);
    open(&mut h);
    h.run_steps(3);

    // Fixed size: about 440 wide and under 350 tall, centered in the window.
    let field = h.state().auto.find("quickEffects.search").expect("the search field").rect;
    let (w, _) = (field[2], field[3]);
    assert!(w > 300.0 && w < 470.0, "the field fits a compact popup, got {w}");

    // Type a query with far more than five matches.
    h.state_mut().ui.quick_effects.query = "blur".into();
    h.run_steps(3);
    let rows: Vec<_> = h.state().auto.elements.iter().filter(|e| e.id.starts_with("quickEffects.item.")).map(|e| e.rect).collect();
    assert!(!rows.is_empty(), "results appear once the user types");
    // The box holds five rows: the visible five plus at most the one partial row `show_rows`
    // renders for smooth scrolling, which the scroll area clips.
    assert!(rows.len() <= 6, "five rows visible (one extra partial row at most), got {}", rows.len());
    // The five-row cap is display-only: the registry holds far more blur effects than fit.
    let blur_matches = effectcraft_engine::effects::all().iter().filter(|e| effectcraft_engine::effects::name_matches(e, "blur")).count();
    assert!(blur_matches > 5, "more than five blur effects exist, got {blur_matches}");

    // The popup box stays the same height no matter how many matches there are.
    let list = h.state().auto.find("quickEffects.list").expect("the results box").rect;
    assert!(list[3] <= 5.0 * 32.0 + 1.0, "the results box holds five compact rows, got {}", list[3]);

    // More matches do not make the popup taller: the box is the same either way.
    let before = list;
    h.state_mut().ui.quick_effects.query = "ec.".into();
    h.run_steps(3);
    let after = h.state().auto.find("quickEffects.list").expect("the results box").rect;
    assert_eq!((before[1], before[3]), (after[1], after[3]), "the popup does not move or grow for a wider result set");
}

#[test]
fn a_click_below_the_visible_popup_closes_it() {
    let mut h = harness(true);
    open(&mut h);
    h.run_steps(4);
    h.state_mut().clock_mut().advance(0.2);
    let field = h.state().auto.find("quickEffects.search").expect("field").rect;
    // With an empty query the frame is just the field plus padding. This point is below that
    // frame but was inside the old forced 220px-tall Area rect, so it did not close the popup.
    let p = pos2(field[0] + field[2] / 2.0, field[1] + 100.0);
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    h.state_mut().clock_mut().advance(0.5);
    h.run_steps(3);
    assert!(!h.state().ui.quick_effects.open, "a click below the visible frame closes the popup");
}

#[test]
fn escape_and_outside_press_close_the_popup() {
    let mut h = harness(true);
    open(&mut h);
    key(&mut h, Key::Escape, Modifiers::NONE);
    h.state_mut().clock_mut().advance(0.5);
    h.run_steps(3);
    assert!(!h.state().ui.quick_effects.open, "Escape closes it");

    open(&mut h);
    h.state_mut().clock_mut().advance(0.2);
    h.input_mut().events.push(Event::PointerButton {
        pos: egui::pos2(5.0, 700.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    h.state_mut().clock_mut().advance(0.5);
    h.run_steps(3);
    assert!(!h.state().ui.quick_effects.open, "an outside press closes it");
}

#[test]
fn recent_and_favorites_are_separate_and_conflicts_are_reported() {
    let mut h = harness(true);
    open(&mut h);
    h.state_mut().ui.quick_effects.query = "Gaussian Blur".into();
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(h.state().ui.effects_recent.len(), 1, "a successful apply records Recent");

    let id = h.state().ui.effects_recent[0].clone();
    assert!(!h.state().session.prefs.is_effect_favorite(&id));
    h.state_mut().session.prefs.toggle_effect_favorite(&id);
    assert!(h.state().session.prefs.is_effect_favorite(&id), "Favorites are independent of Recent");

    let table = h.state().session.shortcuts();
    let b = table.find("effects.quickSearch").expect("the command is bindable");
    assert_eq!(b.path.first().map(String::as_str), Some("Tools & Panels"));
    assert!(table.conflicts(b, "Shift+Space").iter().any(|c| c.key == "playback.preview.shiftSpacebar"));
}
