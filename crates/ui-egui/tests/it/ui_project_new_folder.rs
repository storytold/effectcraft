//! #1658: a folder made from the interface (the Project panel's New Folder button and context
//! menu, File ▸ New ▸ New Folder and its shortcut) opens with its name ready to type, as in After
//! Effects: Enter keeps the typed name, Escape the default one. `engine.execute` (agents, MCP,
//! scripts) keeps the headless default and only creates the folder.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// A comp and an open folder holding a solid, nothing selected.
fn harness(comps: usize) -> (Harness<'static, EffectcraftApp>, ItemId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    for i in 0..comps {
        s.execute("comp.new", json!({"name": format!("Comp {i:03}"), "width": 64, "height": 64, "duration": 1})).unwrap();
    }
    let folder = ItemId(s.execute("project.newFolder", json!({"name": "Bin"})).unwrap()["item"].as_u64().unwrap());
    s.execute("project.select", json!({"items": []})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.project_open_folders.insert(folder.0);
    h.run_steps(3);
    (h, folder)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, button: PointerButton) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn type_text(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    assert!(h.ctx.egui_wants_keyboard_input(), "the name field has the keyboard");
    h.input_mut().events.push(Event::Text(text.into()));
    h.step();
}

/// The folders not in `before`.
fn new_folders(h: &Harness<'_, EffectcraftApp>, before: &[ItemId]) -> Vec<ItemId> {
    h.state().session.project.items.values().filter(|i| i.is_folder() && !before.contains(&i.id)).map(|i| i.id).collect()
}

fn folders(h: &Harness<'_, EffectcraftApp>) -> Vec<ItemId> {
    new_folders(h, &[])
}

fn name(h: &Harness<'_, EffectcraftApp>, id: ItemId) -> Option<String> {
    h.state().session.project.item(id).map(|i| i.name.clone())
}

fn renaming(h: &Harness<'_, EffectcraftApp>, id: ItemId) -> bool {
    let edit = format!("project.item.{}.nameEdit", id.0);
    h.state().auto.elements.iter().any(|e| e.id == edit)
}

/// The panel's button, with a folder selected: the new folder opens its name, Enter keeps the
/// typed one, and undo takes back the rename, then the folder (two steps, like any rename).
#[test]
fn the_new_folder_button_opens_the_name_and_enter_keeps_it() {
    let (mut h, bin) = harness(0);
    let p = rect(&h, &format!("project.item.{}.name", bin.0)).center();
    click_at(&mut h, p, PointerButton::Primary);
    assert_eq!(h.state().session.state.project_selection, [bin]);
    let before = folders(&h);
    let p = rect(&h, "project.newFolder").center();
    click_at(&mut h, p, PointerButton::Primary);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert_eq!(h.state().session.state.project_selection, [folder], "the new folder is selected");
    assert!(renaming(&h, folder), "its name is open for editing");
    type_text(&mut h, "Shots");
    key(&mut h, Key::Enter);
    assert_eq!(name(&h, folder).as_deref(), Some("Shots"), "Enter keeps the typed name");
    assert!(!renaming(&h, folder), "Enter closes the field");
    assert!(!h.ctx.egui_wants_keyboard_input(), "and gives the keyboard back");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(name(&h, folder).as_deref(), Some("Untitled Folder"), "the first undo takes back the rename");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(name(&h, folder), None, "the second, the folder");
}

/// Escape keeps the default name: the folder stays, with one undo step.
#[test]
fn escape_keeps_the_default_name() {
    let (mut h, _) = harness(0);
    let before = folders(&h);
    let p = rect(&h, "project.newFolder").center();
    click_at(&mut h, p, PointerButton::Primary);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    type_text(&mut h, "Never mind");
    key(&mut h, Key::Escape);
    assert_eq!(name(&h, folder).as_deref(), Some("Untitled Folder"));
    assert!(!renaming(&h, folder));
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(name(&h, folder), None, "one undo removes the folder");
}

/// The context menu of the list's empty area.
#[test]
fn the_context_menu_opens_the_name() {
    let (mut h, _) = harness(0);
    let list = rect(&h, "project.empty");
    let before = folders(&h);
    click_at(&mut h, pos2(list.center().x, list.max.y - 4.0), PointerButton::Secondary);
    let p = rect(&h, "project.context.newFolder").center();
    click_at(&mut h, p, PointerButton::Primary);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert!(renaming(&h, folder), "the context menu's New Folder opens the name");
}

/// File ▸ New ▸ New Folder (the menu dispatcher, which its shortcut goes through too): a folder
/// sorted below the visible rows scrolls into view with its name open.
#[test]
fn the_file_menu_opens_the_name_in_view() {
    let (mut h, _) = harness(80);
    let list = rect(&h, "project.empty");
    let before = folders(&h);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "project.newFolder", json!({})).unwrap();
    h.run_steps(3);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert!(renaming(&h, folder), "File ▸ New ▸ New Folder opens the name");
    let field = rect(&h, &format!("project.item.{}.nameEdit", folder.0));
    assert!(list.contains_rect(field), "the field is in view: {field:?} in {list:?}");
    type_text(&mut h, "Renders");
    key(&mut h, Key::Enter);
    assert_eq!(name(&h, folder).as_deref(), Some("Renders"));
}

/// Press on `from`, move to `to` in steps, release there.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn item_named(h: &Harness<'_, EffectcraftApp>, name: &str) -> ItemId {
    h.state().session.project.items.values().find(|i| i.name == name).map(|i| i.id).unwrap_or_else(|| panic!("no item {name}"))
}

fn parent(h: &Harness<'_, EffectcraftApp>, id: ItemId) -> Option<ItemId> {
    h.state().session.project.item(id).and_then(|i| i.parent)
}

/// #3196: two selected items (one at the root, one in a folder) dragged onto the New Folder
/// button go into a new folder, out of the folder they were in, with its name open as the
/// button's; one undo takes it all back.
#[test]
fn items_dropped_on_the_new_folder_button_go_into_a_new_folder() {
    let (mut h, bin) = harness(2);
    let (a, b) = (item_named(&h, "Comp 000"), item_named(&h, "Comp 001"));
    h.state_mut().session.execute("project.move", json!({"items": [b.0], "folder": bin.0})).unwrap();
    h.state_mut().session.state.project_selection = vec![a, b];
    h.run_steps(3);
    let before = folders(&h);
    let project = h.state().session.project.clone();
    let from = rect(&h, &format!("project.item.{}.name", a.0)).center();
    let to = rect(&h, "project.newFolder").center();
    drag(&mut h, from, to);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert_eq!((parent(&h, a), parent(&h, b)), (Some(folder), Some(folder)), "both dragged items are in it");
    assert_eq!(parent(&h, folder), None, "where the button makes folders");
    assert_eq!(h.state().session.state.project_selection, [folder]);
    assert!(renaming(&h, folder), "its name is open, as from the button");
    assert!(h.state().ui.project_open_folders.contains(&folder.0), "open, so the moved items stay in view");
    key(&mut h, Key::Escape);
    assert_eq!(name(&h, folder).as_deref(), Some("Untitled Folder"));
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project, project, "one undo takes back the folder and the move");

    // An item that is not selected goes alone.
    h.state_mut().session.state.project_selection = vec![b];
    h.run_steps(3);
    let before = folders(&h);
    let from = rect(&h, &format!("project.item.{}.name", a.0)).center();
    drag(&mut h, from, to);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert_eq!((parent(&h, a), parent(&h, b)), (Some(folder), Some(bin)), "only the dragged item");
}

/// Agents and scripts: `engine.execute` only creates the folder.
#[test]
fn engine_execute_only_creates_the_folder() {
    let (mut h, _) = harness(0);
    let before = folders(&h);
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new("engine.execute", json!({"command": "project.newFolder"}));
    let now = h.state().now();
    let _ = control::handle(h.state_mut(), &ctx, &req, now);
    h.run_steps(3);
    let [folder] = new_folders(&h, &before)[..] else { panic!("one new folder") };
    assert!(!renaming(&h, folder), "no name field for an agent's folder");
    assert!(!h.ctx.egui_wants_keyboard_input());
}
