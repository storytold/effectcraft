//! #2207: Duplicate is reachable from a Project row's context menu, with the same targets,
//! naming, undo and shortcut preset as Edit > Duplicate.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::menus::{parse_shortcut, shortcut_text};
use egui::{Event, Key, Modifiers, PointerButton, Rect, Shape, pos2};
use egui_kittest::Harness;
use serde_json::json;

const DUPLICATE: &str = "project.context.duplicate";

fn harness() -> (Harness<'static, EffectcraftApp>, ItemId, ItemId, ItemId) {
    let mut s = Session::default();
    let mut comp =
        |name: &str| ItemId(s.execute("comp.new", json!({"name": name, "width": 64, "height": 64, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
    let main = comp("Main");
    let other = comp("Other");
    let folder = ItemId(s.execute("project.newFolder", json!({"name": "Bin"})).unwrap()["item"].as_u64().unwrap());
    s.execute("project.select", json!({"items": []})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, main, other, folder)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str, button: PointerButton, modifiers: Modifiers) {
    let p = rect(h, id).center();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed: true, modifiers });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed: false, modifiers });
    h.run_steps(3);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn row(h: &mut Harness<'_, EffectcraftApp>, item: ItemId, button: PointerButton, modifiers: Modifiers) {
    click(h, &format!("project.item.{}.name", item.0), button, modifiers);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

fn copy_name(h: &Harness<'_, EffectcraftApp>, id: ItemId) -> String {
    h.state().session.project.item(id).unwrap().name.clone()
}

fn menu_text(h: &Harness<'_, EffectcraftApp>, text: &str) -> bool {
    fn contains(shape: &Shape, bounds: Rect, text: &str) -> bool {
        match shape {
            Shape::Text(t) => bounds.contains(t.pos) && t.galley.text() == text,
            Shape::Vec(shapes) => shapes.iter().any(|s| contains(s, bounds, text)),
            _ => false,
        }
    }
    let bounds = rect(h, DUPLICATE);
    h.output().shapes.iter().any(|s| contains(&s.shape, bounds, text))
}

#[test]
fn unselected_row_duplicates_only_itself_and_undo_restores_the_project() {
    let (mut h, main, other, _) = harness();
    row(&mut h, other, PointerButton::Primary, Modifiers::NONE);
    let before = h.state().session.project.clone();
    let undo = h.state().session.history.undo.len();
    row(&mut h, main, PointerButton::Secondary, Modifiers::NONE);
    assert_eq!(h.state().session.state.project_selection, [main]);
    assert!(menu_text(&h, &shortcut_text("Cmd+D")), "the default platform shortcut is shown");
    click(&mut h, DUPLICATE, PointerButton::Primary, Modifiers::NONE);
    let selected = h.state().session.state.project_selection.clone();
    let [copy] = selected.as_slice() else { panic!("one selected copy") };
    assert!(!before.items.contains_key(copy), "the copy has a fresh ID");
    assert_eq!(copy_name(&h, *copy), "Main 2");
    assert_eq!(copy_name(&h, other), "Other");
    assert_eq!(h.state().session.project.items.len(), before.items.len() + 1);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project, before);
}

#[test]
fn selected_rows_duplicate_together_in_selection_order_with_numbered_names() {
    let (mut h, main, other, _) = harness();
    // Deliberately reverse alphabetical row order: command targets retain selection order.
    row(&mut h, other, PointerButton::Primary, Modifiers::NONE);
    row(&mut h, main, PointerButton::Primary, Modifiers::COMMAND);
    assert_eq!(h.state().session.state.project_selection, [other, main]);
    let before = h.state().session.project.clone();
    let undo = h.state().session.history.undo.len();
    row(&mut h, main, PointerButton::Secondary, Modifiers::NONE);
    click(&mut h, DUPLICATE, PointerButton::Primary, Modifiers::NONE);
    let selected = h.state().session.state.project_selection.clone();
    let [a, b] = selected.as_slice() else { panic!("two selected copies") };
    assert_ne!(a, b);
    assert!(selected.iter().all(|id| !before.items.contains_key(id)));
    assert_eq!((copy_name(&h, *a), copy_name(&h, *b)), ("Other 2".to_string(), "Main 2".to_string()));
    assert_eq!(h.state().session.project.items.len(), before.items.len() + 2);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    // Duplicate the selected copies again: the existing engine numbering continues.
    row(&mut h, *b, PointerButton::Secondary, Modifiers::NONE);
    click(&mut h, DUPLICATE, PointerButton::Primary, Modifiers::NONE);
    let names: Vec<_> = h.state().session.state.project_selection.iter().map(|id| copy_name(&h, *id)).collect();
    assert_eq!(names, ["Other 3", "Main 3"]);
}

#[test]
fn folders_and_mixed_selections_disable_duplicate_without_partial_changes() {
    for mixed in [false, true] {
        let (mut h, main, _, folder) = harness();
        row(&mut h, main, PointerButton::Primary, Modifiers::NONE);
        if mixed {
            row(&mut h, folder, PointerButton::Primary, Modifiers::COMMAND);
        }
        // An unselected folder replaces the selection; a selected non-folder keeps the mixed set.
        row(&mut h, if mixed { main } else { folder }, PointerButton::Secondary, Modifiers::NONE);
        let before = h.state().session.project.clone();
        let selected = h.state().session.state.project_selection.clone();
        assert!(selected.contains(&folder));
        let undo = h.state().session.history.undo.len();
        let status = h.state().ui.status.clone();
        click(&mut h, DUPLICATE, PointerButton::Primary, Modifiers::NONE);
        assert_eq!(h.state().session.project, before, "mixed={mixed}: no partial copy");
        assert_eq!(h.state().session.state.project_selection, selected);
        assert_eq!(h.state().session.history.undo.len(), undo);
        assert_eq!(h.state().ui.status, status, "no failing engine command was dispatched");
    }
}

#[test]
fn duplicate_shortcut_still_dispatches_and_the_menu_tracks_rebinding_and_unbinding() {
    let (mut h, main, _, _) = harness();
    row(&mut h, main, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(h.state().ui.focused, PanelKind::Project);
    let (d, command) = parse_shortcut("Cmd+D").unwrap();
    key(&mut h, command, d);
    let selected = h.state().session.state.project_selection.clone();
    let [copy] = selected.as_slice() else { panic!("one shortcut copy") };
    assert_ne!(*copy, main);
    assert_eq!(copy_name(&h, *copy), "Main 2");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    h.state_mut().session.execute("shortcuts.set", json!({"command": "edit.duplicate", "keys": "F6"})).unwrap();
    row(&mut h, main, PointerButton::Secondary, Modifiers::NONE);
    assert!(menu_text(&h, "F6"));
    assert!(!menu_text(&h, &shortcut_text("Cmd+D")));
    key(&mut h, Key::Escape, Modifiers::NONE);
    row(&mut h, main, PointerButton::Primary, Modifiers::NONE);
    let before = h.state().session.project.clone();
    key(&mut h, command, d);
    assert_eq!(h.state().session.project, before, "the former shortcut stays unbound");
    key(&mut h, Key::F6, Modifiers::NONE);
    assert_eq!(h.state().session.project.items.len(), before.items.len() + 1);
    h.state_mut().session.execute("shortcuts.set", json!({"command": "edit.duplicate", "keys": null})).unwrap();
    row(&mut h, main, PointerButton::Secondary, Modifiers::NONE);
    assert!(!menu_text(&h, "F6"));
    assert!(!menu_text(&h, &shortcut_text("Cmd+D")));
    click(&mut h, DUPLICATE, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(h.state().session.project.items.len(), before.items.len() + 2, "unbinding keeps the menu action usable");
}
