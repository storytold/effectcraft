//! Project-panel deletion uses its selection, even when a Timeline layer is still selected.
//! Shift+Delete deletes it without asking (Delete Project Items Without Confirmation).

use effectcraft_engine::Session;
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind, LayerId};
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, Vec<ItemId>, LayerId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Delete me", "width": 320, "height": 180, "duration": 4})).unwrap();
    let comp = s.active_comp_id().unwrap();
    s.execute("comp.new", json!({"name": "Working composition", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap();
    let solid = s.project.items.values().find(|i| matches!(i.kind, ItemKind::Solid(_))).unwrap().id;
    let footage = std::sync::Arc::make_mut(&mut s.project).add_item(
        "Data asset",
        Default::default(),
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Data, path: "original.json".into(), data: Some("{}".into()), ..Default::default() }),
    );
    let folder = ItemId(s.execute("project.newFolder", json!({"name": "Folder"})).unwrap()["item"].as_u64().unwrap());
    let keep = LayerId(s.execute("layer.newNull", json!({"name": "Keep this layer"})).unwrap()["layer"].as_u64().unwrap());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    let folders: Vec<u64> = h.state().session.project.items.values().filter(|i| i.is_folder()).map(|i| i.id.0).collect();
    h.state_mut().ui.project_open_folders.extend(folders);
    h.run_steps(3);
    (h, vec![comp, solid, footage, folder], keep)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str, modifiers: Modifiers) {
    // Let a dialog opened by the previous input finish its first (sizing) frame, so its buttons
    // are where the automation tree says.
    h.run_steps(2);
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let p = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

/// Shift+Delete as each platform delivers it: a key press (macOS, Linux), or on Windows a Cut
/// event with Shift held (the windowing layer turns Shift+Delete into Cut).
#[derive(Clone, Copy, Debug)]
enum ShiftDelete {
    Key,
    WindowsCut,
}

fn shift_delete(h: &mut Harness<'_, EffectcraftApp>, how: ShiftDelete) {
    match how {
        ShiftDelete::Key => key(h, Key::Delete, Modifiers::SHIFT),
        ShiftDelete::WindowsCut => {
            h.input_mut().events.push(Event::ModifiersChanged(Modifiers::SHIFT));
            h.step();
            h.input_mut().events.push(Event::Cut);
            h.step();
            h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
            h.run_steps(2);
        }
    }
}

const SHIFT_DELETE: [ShiftDelete; 2] = [ShiftDelete::Key, ShiftDelete::WindowsCut];

/// Items that compositions use ask first, as in After Effects: answer Delete when asked.
/// Returns whether it asked.
fn confirm_if_asked(h: &mut Harness<'_, EffectcraftApp>) -> bool {
    if h.state().dialog != Some(Dialog::DeleteItems) {
        return false;
    }
    click(h, "dialog.deleteItems.delete", Modifiers::NONE);
    true
}

#[test]
fn delete_and_backspace_remove_project_items_and_undo() {
    for delete_key in [Key::Delete, Key::Backspace] {
        for target in 0..4 {
            let (mut h, items, keep) = harness();
            let item = items[target];
            click(&mut h, &format!("project.item.{}.name", item.0), Modifiers::NONE);
            assert_eq!(h.state().ui.focused, PanelKind::Project);
            assert_eq!(h.state().session.state.project_selection, vec![item]);
            assert_eq!(h.state().session.state.selected_layers, vec![keep]);
            let project = h.state().session.project.clone();
            let undo = h.state().session.history.undo.len();
            key(&mut h, delete_key, Modifiers::NONE);
            // Only the solid is used by a layer: only it asks first.
            assert_eq!(confirm_if_asked(&mut h), target == 1, "item {target}");
            assert!(h.state().session.project.item(item).is_none(), "{delete_key:?} should delete project item {target}");
            assert!(h.state().session.active_comp().unwrap().layer(keep).is_some(), "the unrelated Timeline layer stays");
            assert!(h.state().session.state.project_selection.is_empty());
            assert_eq!(h.state().session.project.items.len(), project.items.len() - 1);
            assert_eq!(h.state().session.history.undo.len(), undo + 1);
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(h.state().session.project, project, "one undo restores the item and its references");
        }
    }
}

#[test]
fn project_multi_selection_deletes_together_and_undoes_once() {
    let (mut h, items, keep) = harness();
    for (i, item) in items.iter().enumerate() {
        click(&mut h, &format!("project.item.{}.name", item.0), if i == 0 { Modifiers::NONE } else { Modifiers::COMMAND });
    }
    assert_eq!(h.state().session.state.project_selection, items);
    let project = h.state().session.project.clone();
    let undo = h.state().session.history.undo.len();
    key(&mut h, Key::Backspace, Modifiers::NONE);
    assert!(confirm_if_asked(&mut h), "the solid is in use");
    assert!(items.iter().all(|i| h.state().session.project.item(*i).is_none()));
    assert!(h.state().session.active_comp().unwrap().layer(keep).is_some());
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project, project);
}

#[test]
fn empty_project_selection_does_not_delete_a_timeline_layer() {
    let (mut h, items, keep) = harness();
    click(&mut h, &format!("project.item.{}.name", items[0].0), Modifiers::NONE);
    h.state_mut().session.execute("project.select", json!({"items": []})).unwrap();
    let undo = h.state().session.history.undo.len();
    for delete_key in [Key::Delete, Key::Backspace] {
        key(&mut h, delete_key, Modifiers::NONE);
        assert!(h.state().session.active_comp().unwrap().layer(keep).is_some());
        assert_eq!(h.state().session.history.undo.len(), undo);
    }
}

#[test]
fn viewer_focus_keeps_layer_deletion_with_a_project_selection() {
    for delete_key in [Key::Delete, Key::Backspace] {
        let (mut h, items, keep) = harness();
        click(&mut h, &format!("project.item.{}.name", items[0].0), Modifiers::NONE);
        click(&mut h, "viewer.comp", Modifiers::NONE);
        h.state_mut().session.execute("layer.select", json!({"layers": [keep.0]})).unwrap();
        assert_eq!(h.state().ui.focused, PanelKind::Composition);
        key(&mut h, delete_key, Modifiers::NONE);
        assert!(h.state().session.active_comp().unwrap().layer(keep).is_none());
        assert!(h.state().session.project.item(items[0]).is_some());
    }
}

#[test]
fn typing_dialogs_and_modified_keys_do_not_delete_project_items() {
    let (mut h, items, _) = harness();
    let item = items[0];
    click(&mut h, &format!("project.item.{}.name", item.0), Modifiers::NONE);
    let project = h.state().session.project.clone();
    // Only plain Delete/Backspace and Shift+Delete delete Project items.
    for (k, m) in [
        (Key::Backspace, Modifiers::SHIFT),
        (Key::Delete, Modifiers::ALT),
        (Key::Delete, Modifiers::COMMAND),
        (Key::Delete, Modifiers::SHIFT | Modifiers::COMMAND),
        (Key::Delete, Modifiers::SHIFT | Modifiers::ALT),
    ] {
        key(&mut h, k, m);
        assert_eq!(h.state().session.project, project, "{m:?}+{k:?}");
    }
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    key(&mut h, Key::Delete, Modifiers::NONE);
    for how in SHIFT_DELETE {
        shift_delete(&mut h, how);
    }
    assert_eq!(h.state().session.project, project);
    h.state_mut().dialog = None;
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.run_steps(2);
    assert!(h.ctx.egui_wants_keyboard_input(), "the inline rename field has focus");
    for delete_key in [Key::Backspace, Key::Delete] {
        key(&mut h, delete_key, Modifiers::NONE);
        assert_eq!(h.state().session.project, project, "typing must not delete the item");
    }
    for how in SHIFT_DELETE {
        shift_delete(&mut h, how);
        assert_eq!(h.state().session.project, project, "typing must not delete the item ({how:?})");
    }
}

/// Deleting an item a composition uses asks first, as in After Effects, and says what goes with
/// it; Cancel and Escape keep everything, Delete removes the item and the layers using it in one
/// undo step. Agents running the command directly are not asked.
#[test]
fn deleting_items_in_use_asks_first() {
    let (mut h, items, _) = harness();
    let solid = items[1];
    let usage = h.state_mut().session.execute("project.usage", json!({"items": [solid.0]})).unwrap();
    assert_eq!(usage, json!({"items": 1, "layers": 1, "comps": 1}));
    let uses = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.project.comps().flat_map(|(_, c)| c.layers.iter()).filter(|l| l.source.item() == Some(solid)).count()
    };
    click(&mut h, &format!("project.item.{}.name", solid.0), Modifiers::NONE);
    for cancel in ["button", "escape"] {
        key(&mut h, Key::Delete, Modifiers::NONE);
        assert_eq!(h.state().dialog, Some(Dialog::DeleteItems));
        if cancel == "button" {
            click(&mut h, "dialog.deleteItems.cancel", Modifiers::NONE);
        } else {
            key(&mut h, Key::Escape, Modifiers::NONE);
        }
        assert_eq!(h.state().dialog, None, "{cancel}");
        assert!(h.state().session.project.item(solid).is_some() && uses(&h) == 1, "{cancel} keeps the item and its layer");
    }
    let undo = h.state().session.history.undo.len();
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert!(confirm_if_asked(&mut h));
    assert!(h.state().session.project.item(solid).is_none() && uses(&h) == 0);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    // The command itself (agents, scripts) deletes without asking.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.state_mut().session.execute("project.delete", json!({"items": [solid.0]})).unwrap();
    assert!(h.state().session.project.item(solid).is_none());
}

/// Shift+Delete deletes the selected item without asking, even one a composition uses, with the
/// layers that use it, in one undo step (Delete Project Items Without Confirmation).
#[test]
fn shift_delete_deletes_without_asking_and_undoes_once() {
    for how in SHIFT_DELETE {
        for target in 0..4 {
            let (mut h, items, keep) = harness();
            let item = items[target];
            click(&mut h, &format!("project.item.{}.name", item.0), Modifiers::NONE);
            let project = h.state().session.project.clone();
            let undo = h.state().session.history.undo.len();
            shift_delete(&mut h, how);
            assert_eq!(h.state().dialog, None, "{how:?} item {target}: no prompt");
            assert!(h.state().session.project.item(item).is_none(), "{how:?} should delete project item {target}");
            assert!(h.state().session.active_comp().unwrap().layer(keep).is_some(), "the unrelated Timeline layer stays");
            assert!(h.state().session.state.project_selection.is_empty());
            assert_eq!(h.state().session.history.undo.len(), undo + 1);
            h.state_mut().session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(h.state().session.project, project, "one undo restores the item and its references");
        }
    }
}

#[test]
fn shift_delete_deletes_a_multi_selection_and_folder_contents_together() {
    for how in SHIFT_DELETE {
        let (mut h, items, keep) = harness();
        let (solid, folder) = (items[1], items[3]);
        // The solid (used by a layer) inside the folder: deleting the folder takes both.
        h.state_mut().session.execute("project.move", json!({"items": [solid.0], "folder": folder.0})).unwrap();
        h.run_steps(2);
        click(&mut h, &format!("project.item.{}.name", items[0].0), Modifiers::NONE);
        click(&mut h, &format!("project.item.{}.name", folder.0), Modifiers::COMMAND);
        assert_eq!(h.state().session.state.project_selection, vec![items[0], folder]);
        let project = h.state().session.project.clone();
        let undo = h.state().session.history.undo.len();
        shift_delete(&mut h, how);
        assert_eq!(h.state().dialog, None, "{how:?}: no prompt");
        for gone in [items[0], solid, folder] {
            assert!(h.state().session.project.item(gone).is_none(), "{how:?}: {gone:?} goes");
        }
        assert!(h.state().session.project.item(items[2]).is_some(), "the unselected item stays");
        let uses = h.state().session.project.comps().flat_map(|(_, c)| c.layers.iter()).filter(|l| l.source.item() == Some(solid)).count();
        assert_eq!(uses, 0, "the layer using the solid goes with it");
        assert!(h.state().session.active_comp().unwrap().layer(keep).is_some());
        assert_eq!(h.state().session.history.undo.len(), undo + 1);
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(h.state().session.project, project);
    }
}

/// Shift+Delete with nothing selected deletes nothing, and skipping the prompt once (Shift+Delete,
/// or answering Delete) doesn't skip it for the next Delete.
#[test]
fn shift_delete_and_answered_prompts_dont_skip_the_next_prompt() {
    for how in SHIFT_DELETE {
        let (mut h, items, keep) = harness();
        let solid = items[1];
        click(&mut h, &format!("project.item.{}.name", items[0].0), Modifiers::NONE);
        h.state_mut().session.execute("project.select", json!({"items": []})).unwrap();
        let project = h.state().session.project.clone();
        shift_delete(&mut h, how);
        assert_eq!(h.state().dialog, None);
        assert_eq!(h.state().session.project, project, "{how:?} with nothing selected deletes nothing");
        assert!(h.state().session.active_comp().unwrap().layer(keep).is_some());

        // Without asking once, then Delete on an item in use still asks.
        click(&mut h, &format!("project.item.{}.name", items[2].0), Modifiers::NONE);
        shift_delete(&mut h, how);
        assert!(h.state().session.project.item(items[2]).is_none());
        click(&mut h, &format!("project.item.{}.name", solid.0), Modifiers::NONE);
        key(&mut h, Key::Delete, Modifiers::NONE);
        assert_eq!(h.state().dialog, Some(Dialog::DeleteItems), "{how:?}: Delete still asks after Shift+Delete");
        click(&mut h, "dialog.deleteItems.cancel", Modifiers::NONE);
        assert!(h.state().session.project.item(solid).is_some());
    }

    // Answering Delete runs that deletion only: the next Delete asks again.
    let (mut h, items, _) = harness();
    let solid = items[1];
    h.state_mut().session.execute("layer.newSolid", json!({"name": "Second solid", "color": "#806040"})).unwrap();
    let second = h.state().session.project.items.values().find(|i| i.name == "Second solid").unwrap().id;
    h.run_steps(2);
    click(&mut h, &format!("project.item.{}.name", solid.0), Modifiers::NONE);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert!(confirm_if_asked(&mut h));
    assert!(h.state().session.project.item(solid).is_none());
    click(&mut h, &format!("project.item.{}.name", second.0), Modifiers::NONE);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(h.state().dialog, Some(Dialog::DeleteItems), "the answer was for the first deletion only");
}

/// Shift+Delete only deletes Project items with the Project panel focused.
#[test]
fn shift_delete_in_other_panels_keeps_project_items() {
    for how in SHIFT_DELETE {
        let (mut h, items, _) = harness();
        click(&mut h, &format!("project.item.{}.name", items[0].0), Modifiers::NONE);
        click(&mut h, "viewer.comp", Modifiers::NONE);
        assert_eq!(h.state().ui.focused, PanelKind::Composition);
        assert_eq!(h.state().session.state.project_selection, vec![items[0]]);
        shift_delete(&mut h, how);
        assert_eq!(h.state().dialog, None);
        assert!(items.iter().all(|i| h.state().session.project.item(*i).is_some()), "{how:?} in the Composition panel");
    }
}
