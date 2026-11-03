//! Project panel keyboard navigation and accessibility (#776): the arrow keys move and extend the
//! selection and open / close folders, as in After Effects; the rows, sort headers and row buttons
//! reach screen readers with names, roles and states; a row focused with Tab keeps the arrows, and
//! keyboard focus follows the selection.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemId, ItemKind};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

/// A closed folder holding a comp, and a solid at the project root: (harness, folder, comp, solid).
fn harness() -> (Harness<'static, EffectcraftApp>, ItemId, ItemId, ItemId) {
    let mut s = Session::default();
    let comp = ItemId(s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap());
    s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap();
    let solid = s.project.items.values().find(|i| matches!(i.kind, ItemKind::Solid(_))).unwrap().id;
    let folder = ItemId(s.execute("project.newFolder", json!({"name": "Folder"})).unwrap()["item"].as_u64().unwrap());
    s.execute("project.move", json!({"items": [comp.0], "folder": folder.0})).unwrap();
    s.execute("project.move", json!({"items": [solid.0], "folder": null})).unwrap();
    s.execute("project.select", json!({"items": []})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, folder, comp, solid)
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

/// Press and release `key` with `modifiers` (one frame each), then let the UI settle.
fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.run_steps(2);
}

/// The item rows on screen, top to bottom.
fn rows(h: &Harness<'_, EffectcraftApp>) -> Vec<ItemId> {
    let mut rows: Vec<(f32, ItemId)> = h
        .state()
        .auto
        .query("project.item.")
        .into_iter()
        .filter_map(|e| e.id.strip_prefix("project.item.").and_then(|n| n.parse().ok()).map(|n| (e.rect[1], ItemId(n))))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, id)| id).collect()
}

fn selection(h: &Harness<'_, EffectcraftApp>) -> Vec<ItemId> {
    h.state().session.state.project_selection.clone()
}

#[test]
fn arrow_keys_move_and_extend_the_project_selection() {
    let (mut h, _, _, _) = harness();
    let rows = rows(&h);
    assert!(rows.len() >= 2, "rows: {rows:?}");
    let (first, second) = (rows[0], rows[1]);
    click(&mut h, &format!("project.item.{}.name", first.0));
    assert_eq!(h.state().ui.focused, PanelKind::Project);
    assert_eq!(selection(&h), vec![first]);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![second], "Down Arrow selects the next item");
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selection(&h), vec![first], "Up Arrow selects the previous item");
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selection(&h), vec![first], "the first item stays selected at the top");
    for _ in 0..rows.len() + 1 {
        key(&mut h, Key::ArrowDown, Modifiers::SHIFT);
    }
    assert_eq!(selection(&h), rows, "Shift+Down Arrow extends the selection to the last item");
    key(&mut h, Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(selection(&h), rows[..rows.len() - 1], "Shift+Up Arrow shrinks it back toward the anchor");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![rows[rows.len() - 1]], "a plain arrow selects one item again");
    // With nothing selected, Down selects the first item and Up the last.
    h.state_mut().session.state.project_selection.clear();
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![first]);
    h.state_mut().session.state.project_selection.clear();
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selection(&h), vec![rows[rows.len() - 1]]);
}

#[test]
fn right_and_left_arrows_open_and_close_the_selected_folder() {
    let (mut h, folder, comp, _) = harness();
    click(&mut h, &format!("project.item.{}.name", folder.0));
    assert!(!rows(&h).contains(&comp), "the folder starts closed");
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert!(h.state().ui.project_open_folders.contains(&folder.0), "Right Arrow opens the selected folder");
    assert!(rows(&h).contains(&comp));
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![comp], "Down Arrow walks into the open folder");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(selection(&h), vec![folder], "Left Arrow on an item in a folder selects the folder");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert!(!h.state().ui.project_open_folders.contains(&folder.0), "Left Arrow closes the selected folder");
    assert_eq!(selection(&h), vec![folder]);
}

#[test]
fn arrows_leave_the_project_selection_alone_when_another_panel_has_focus() {
    let (mut h, folder, _, _) = harness();
    click(&mut h, &format!("project.item.{}.name", folder.0));
    h.state_mut().ui.focused = PanelKind::Timeline;
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(selection(&h), vec![folder]);
    assert!(!h.state().ui.project_open_folders.contains(&folder.0));
}

#[test]
fn rows_and_headers_have_accessible_names_roles_and_states() {
    let (mut h, folder, _, _) = harness();
    click(&mut h, &format!("project.item.{}.name", folder.0));
    let row = h.get_by_label("Folder, Folder");
    let node = row.accesskit_node();
    assert_eq!(node.role(), Role::TreeItem);
    assert_eq!(node.is_selected(), Some(true), "the selected row says so");
    assert_eq!(node.data().is_expanded(), Some(false), "a closed folder says so");
    assert_eq!(node.data().level(), Some(1));
    let solid = h.get_by_label("Solid, Solid");
    assert_eq!(solid.accesskit_node().is_selected(), Some(false));
    assert_eq!(solid.accesskit_node().data().is_expanded(), None, "only folders open");
    assert_eq!(h.get_by_label("Sort by Name, ascending").accesskit_node().role(), Role::Button);
    assert_eq!(h.get_by_label("Open folder Folder").accesskit_node().role(), Role::Button);
    assert_eq!(h.get_by_label("Delete selected project items").accesskit_node().role(), Role::Button);
    assert!(h.query_by_label_contains("Label of Folder: ").is_some(), "the label swatch is named");
    assert!(h.query_by_label("Create a new Folder").is_some());
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.get_by_label("Folder, Folder").accesskit_node().data().is_expanded(), Some(true));
    assert_eq!(h.get_by_label("Main, Composition").accesskit_node().data().level(), Some(2), "the comp sits one level down");
}

#[test]
fn a_focused_row_keeps_the_arrows_and_focus_follows_the_selection() {
    let (mut h, _, _, _) = harness();
    let rows = rows(&h);
    let names: Vec<String> = rows
        .iter()
        .map(|id| {
            let it = h.state().session.project.item(*id).unwrap();
            format!("{}, {}", it.name, it.type_name())
        })
        .collect();
    // Focus the first row the way a screen reader (or Tab) does, without a click: the panel
    // takes focus with it.
    h.state_mut().ui.focused = PanelKind::Timeline;
    h.get_by_label(&names[0]).focus();
    h.run_steps(3);
    assert!(h.get_by_label(&names[0]).is_focused());
    assert_eq!(h.state().ui.focused, PanelKind::Project, "keyboard focus on a row focuses the panel");
    // A plain arrow moved egui's focus to the widget below; here it moves the selection and the
    // focus with it.
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![rows[0]], "nothing was selected: Down selects the first item");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selection(&h), vec![rows[1]]);
    assert!(h.get_by_label(&names[1]).is_focused(), "keyboard focus follows the selection");
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selection(&h), vec![rows[0]]);
    assert!(h.get_by_label(&names[0]).is_focused());
}

/// A mouse click on a row gives no keyboard focus to the row, so the app's shortcuts run as
/// before the arrow keys came: Delete / Backspace delete, Cmd+D duplicates, Enter renames, also
/// after an arrow key moved the selection.
#[test]
fn shortcuts_still_run_after_a_mouse_click_and_the_arrows() {
    let focus = |h: &Harness<'_, EffectcraftApp>| h.ctx.memory(|m| m.focused());
    for delete_key in [Key::Delete, Key::Backspace] {
        let (mut h, folder, _, _) = harness();
        let empty = ItemId(h.state_mut().session.execute("project.newFolder", json!({"name": "Empty"})).unwrap()["item"].as_u64().unwrap());
        h.run_steps(2);
        click(&mut h, &format!("project.item.{}.name", empty.0));
        assert_eq!(selection(&h), vec![empty]);
        assert_eq!(focus(&h), None, "a click gives the row no keyboard focus");
        key(&mut h, delete_key, Modifiers::NONE);
        assert!(h.state().session.project.item(empty).is_none(), "{delete_key:?} deletes the clicked item");
        // Click, an arrow, then the shortcut.
        let empty2 = ItemId(h.state_mut().session.execute("project.newFolder", json!({"name": "Empty 2"})).unwrap()["item"].as_u64().unwrap());
        h.run_steps(2);
        let rows = rows(&h);
        let at = rows.iter().position(|r| *r == empty2).unwrap();
        let from = if at > 0 { rows[at - 1] } else { rows[at + 1] };
        click(&mut h, &format!("project.item.{}.name", from.0));
        key(&mut h, if at > 0 { Key::ArrowDown } else { Key::ArrowUp }, Modifiers::NONE);
        assert_eq!(selection(&h), vec![empty2]);
        assert_eq!(focus(&h), None, "the arrows don't take keyboard focus after a click");
        key(&mut h, delete_key, Modifiers::NONE);
        assert!(h.state().session.project.item(empty2).is_none(), "{delete_key:?} deletes the item the arrow moved to");
        assert!(h.state().session.project.item(folder).is_some());
    }
    let (mut h, folder, _, solid) = harness();
    // Folders can't be duplicated (engine rule): duplicate the solid.
    click(&mut h, &format!("project.item.{}.name", solid.0));
    let n = h.state().session.project.items.len();
    key(&mut h, Key::D, Modifiers::COMMAND);
    assert_eq!(h.state().session.project.items.len(), n + 1, "Cmd+D duplicates the clicked item");
    click(&mut h, &format!("project.item.{}.name", folder.0));
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().auto.find(&format!("project.item.{}.nameEdit", folder.0)).is_some(), "Enter renames the clicked item");
}
