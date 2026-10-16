//! Project panel rows: a folder's twirl and an item's label swatch take their own clicks, over
//! the row's click / drag area (#152).

use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Color32, Event, Modifiers, Pos2, Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

fn button(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    button(h, p, true);
    h.step();
    button(h, p, false);
    h.run_steps(3);
}

/// Both clicks in one frame: the harness' frame time is longer than a double-click's.
fn double_click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for _ in 0..2 {
        button(h, p, true);
        button(h, p, false);
    }
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    click_at(h, p);
}

#[test]
fn command_clicking_a_folder_twirl_opens_the_folders_inside_it() {
    let (mut h, folder, _, _) = harness();
    let s = &mut h.state_mut().session;
    let inner = s.execute("project.newFolder", json!({"name": "Inner"})).unwrap()["item"].as_u64().unwrap();
    let deep = s.execute("project.newFolder", json!({"name": "Deep"})).unwrap()["item"].as_u64().unwrap();
    s.execute("project.move", json!({"items": [inner], "folder": folder.0})).unwrap();
    s.execute("project.move", json!({"items": [deep], "folder": inner})).unwrap();
    s.execute("project.select", json!({"items": []})).unwrap();
    h.state_mut().ui.project_open_folders.clear();
    h.run_steps(3);
    let p = center(&h, &format!("project.item.{}.twirl", folder.0));
    let cmd_click = |h: &mut Harness<'_, EffectcraftApp>| {
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
        click_at(h, p);
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
        h.step();
    };
    cmd_click(&mut h);
    let open = h.state().ui.project_open_folders.clone();
    assert!([folder.0, inner, deep].iter().all(|f| open.contains(f)), "the folder and every folder inside it open");
    cmd_click(&mut h);
    assert!(h.state().ui.project_open_folders.is_empty(), "and close again together");
}

#[test]
fn folder_twirl_opens_and_closes_the_folder_on_one_click() {
    let (mut h, folder, comp, _) = harness();
    let twirl = format!("project.item.{}.twirl", folder.0);
    let comp_row = format!("project.item.{}", comp.0);
    assert!(!h.state().ui.project_open_folders.contains(&folder.0));
    assert!(h.state().auto.find(&comp_row).is_none(), "the closed folder hides its comp");
    click(&mut h, &twirl);
    assert!(h.state().ui.project_open_folders.contains(&folder.0), "one click on the twirl opens the folder");
    assert!(h.state().auto.find(&comp_row).is_some(), "the open folder shows its comp");
    assert!(h.state().session.state.project_selection.is_empty(), "the twirl does not select the folder");
    click(&mut h, &twirl);
    assert!(!h.state().ui.project_open_folders.contains(&folder.0), "a second click closes it");
    // The row under the twirl still selects on a click and twirls on a double-click.
    let name = center(&h, &format!("project.item.{}.name", folder.0));
    click_at(&mut h, name);
    assert_eq!(h.state().session.state.project_selection, vec![folder]);
    double_click_at(&mut h, name);
    assert!(h.state().ui.project_open_folders.contains(&folder.0), "a double-click on the row opens the folder");
}

/// A swatch (a small rect filled with `col`) painted this frame in the left half of `row`.
fn swatch_in(h: &Harness<'_, EffectcraftApp>, row: Rect, col: Color32) -> bool {
    fn walk(s: &egui::Shape, row: Rect, col: Color32) -> bool {
        match s {
            egui::Shape::Rect(r) => r.fill == col && r.rect.width() < 16.0 && row.contains(r.rect.center()) && r.rect.center().x < row.center().x,
            egui::Shape::Vec(v) => v.iter().any(|s| walk(s, row, col)),
            _ => false,
        }
    }
    h.output().shapes.iter().any(|c| walk(&c.shape, row, col))
}

#[test]
fn label_swatch_opens_the_label_menu() {
    let (mut h, _, _, solid) = harness();
    assert_ne!(h.state().session.project.item(solid).unwrap().label, Label::Green);
    let green = h.state().session.prefs.label_name(Label::Green);
    assert!(h.query_by_label(&green).is_none());
    click(&mut h, &format!("project.item.{}.label", solid.0));
    assert!(h.state().session.state.project_selection.is_empty(), "the swatch does not select the row");
    let entry = h.query_by_label(&green).expect("the label menu is open").rect();
    // Each entry shows its colour before its name (#290).
    assert!(swatch_in(&h, entry, h.state().tokens.label(Label::Green)), "no {green} swatch");
    let undo = h.state().session.history.undo.len();
    click_at(&mut h, entry.center());
    assert_eq!(h.state().session.project.item(solid).unwrap().label, Label::Green);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    assert!(h.query_by_label(&green).is_none(), "the menu closed after the choice");
}

/// Right-clicking a footage item selects it and offers the File menu's footage commands:
/// Interpret Footage ▸ Main... opens the Interpret Footage dialog on it (#290). A solid has none.
#[test]
fn footage_context_menu_interprets_the_footage() {
    let (mut h, _, _, solid) = harness();
    let footage = Footage { kind: FootageKind::Video, path: "clip.mp4".into(), width: 64, height: 36, has_video: true, ..Default::default() };
    let clip = std::sync::Arc::make_mut(&mut h.state_mut().session.project).add_item("Clip", Default::default(), None, ItemKind::Footage(footage));
    h.run_steps(2);
    let right_click = |h: &mut Harness<'_, EffectcraftApp>, p: Pos2| {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    // The menu's Interpret Footage entry, not the panel's footer button of the same name.
    let menu_entry = |h: &Harness<'_, EffectcraftApp>| {
        let footer = center(h, "project.interpret");
        h.query_all_by_label_contains("Interpret Footage").find(|n| !n.rect().contains(footer)).map(|n| n.rect())
    };
    let at = center(&h, &format!("project.item.{}.name", solid.0));
    right_click(&mut h, at);
    assert_eq!(h.state().session.state.project_selection, vec![solid]);
    assert!(menu_entry(&h).is_none(), "a solid isn't footage");
    h.input_mut().events.push(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let at = center(&h, &format!("project.item.{}.name", clip.0));
    right_click(&mut h, at);
    assert_eq!(h.state().session.state.project_selection, vec![clip], "right-click selects the item");
    let sub = menu_entry(&h).expect("Interpret Footage in the menu");
    h.input_mut().events.push(Event::PointerMoved(sub.center()));
    h.run_steps(3);
    let main = h.query_by_label_contains("Main...").expect("Interpret Footage ▸ Main...").rect();
    for k in 0..=6 {
        h.input_mut().events.push(Event::PointerMoved(pos2(main.center().x, sub.center().y + (main.center().y - sub.center().y) * k as f32 / 6.0)));
        h.step();
    }
    click_at(&mut h, main.center());
    assert_eq!(h.state().dialog, Some(Dialog::Form), "the Interpret Footage dialog opened");
}

/// #360: the row menu's New Comp from Selection works on the whole selection, as File ▸ New
/// Comp from Selection does: with several items selected it asks how in After Effects' New
/// Composition from Selection dialog, and Multiple Compositions makes one per item.
#[test]
fn context_menu_new_comp_from_selection_uses_the_selection() {
    let (mut h, _, comp, solid) = harness();
    h.state_mut().session.state.project_selection = vec![comp, solid];
    h.run_steps(2);
    let at = center(&h, &format!("project.item.{}.name", solid.0));
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert_eq!(h.state().session.state.project_selection, vec![comp, solid], "the selection stays");
    let item = h.query_by_label_contains("New Comp from Selection").expect("in the menu").rect();
    click_at(&mut h, item.center());
    assert_eq!(h.state().dialog, Some(Dialog::Form));
    assert!(h.state().auto.find("form.field.single").is_some(), "the New Composition from Selection dialog");
    let comps = h.state().session.project.comps().count();
    h.state_mut().session.execute_checked("file.newCompFromSelection", json!({"single": false})).unwrap();
    assert_eq!(h.state().session.project.comps().count(), comps + 2, "one per item");
}

/// Dragging from the Project panel's empty area draws a selection box that selects the rows it
/// touches; a click there deselects (#203).
#[test]
fn dragging_in_the_empty_area_box_selects_items() {
    let (mut h, folder, _, solid) = harness();
    let list = h.state().auto.find("project.empty").unwrap().rect;
    let from = pos2(list[0] + 40.0, list[1] + list[3] - 6.0);
    let to = center(&h, &format!("project.item.{}.name", folder.0));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    button(&mut h, from, true);
    h.step();
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        h.step();
    }
    button(&mut h, to, false);
    h.run_steps(2);
    let sel = h.state().session.state.project_selection.clone();
    assert!(sel.contains(&folder) && sel.contains(&solid), "{sel:?}");
    click_at(&mut h, from);
    assert!(h.state().session.state.project_selection.is_empty());
}

/// A Project panel shorter than its scroll bar's thumb draws instead of panicking: an empty
/// project in a 1280 × 800 browser window at device pixel ratio 2 (640 × 400 points) left the
/// list too short for the thumb's 16-point minimum (#231).
#[test]
fn a_short_project_panel_draws() {
    for height in [400.0, 300.0] {
        let mut h = Harness::builder().with_size(egui::vec2(640.0, height)).build_eframe(|_| EffectcraftApp::new(Session::default()));
        h.run_steps(3);
        assert!(h.state().auto.find("project.empty").is_some(), "the Project panel drew at {height} points");
    }
}

/// #1058: a comp's row menu offers Composition Settings…, as After Effects' does, for the comp
/// right-clicked even when the viewer shows another one; OK applies to that comp. Footage has none.
#[test]
fn comp_context_menu_opens_that_comps_settings() {
    let (mut h, _, main, _) = harness();
    let s = &mut h.state_mut().session;
    let side = ItemId(s.execute("comp.new", json!({"name": "Side", "width": 640, "height": 360, "duration": 2})).unwrap()["comp"].as_u64().unwrap());
    s.execute("comp.open", json!({"comp": main.0})).unwrap();
    let footage = Footage { kind: FootageKind::Video, path: "clip.mp4".into(), width: 64, height: 36, has_video: true, ..Default::default() };
    let clip = std::sync::Arc::make_mut(&mut s.project).add_item("Clip", Default::default(), None, ItemKind::Footage(footage));
    h.run_steps(2);
    assert_eq!(h.state().session.active_comp_id(), Some(main));
    let right_click = |h: &mut Harness<'_, EffectcraftApp>, p: Pos2| {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    let at = center(&h, &format!("project.item.{}.name", clip.0));
    right_click(&mut h, at);
    assert!(h.query_by_label("Rename").is_some(), "the footage menu is open");
    assert!(h.query_by_label_contains("Composition Settings").is_none(), "footage isn't a comp");
    h.input_mut().events.push(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let at = center(&h, &format!("project.item.{}.name", side.0));
    right_click(&mut h, at);
    let entry = h.query_by_label_contains("Composition Settings").expect("in the comp's menu").rect();
    click_at(&mut h, entry.center());
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
    assert_eq!(h.state().session.active_comp_id(), Some(main), "the viewer keeps its comp");
    // A new name, then OK: Side's own size goes back to Side under it. Main's values in the
    // dialog, or OK writing to the viewer's comp, would resize or rename the wrong one.
    click(&mut h, "dialog.comp.name");
    h.input_mut().events.push(Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.input_mut().events.push(Event::Text("Side 2".into()));
    h.run_steps(2);
    let undo = h.state().session.history.undo.len();
    h.input_mut().events.push(Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialog, None, "OK closed the dialog");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "OK applied the settings");
    let size = |h: &Harness<'_, EffectcraftApp>, c: ItemId| {
        let s = &h.state().session;
        (s.project.item(c).unwrap().name.clone(), s.project.comp(c).unwrap().width, s.project.comp(c).unwrap().height)
    };
    assert_eq!(size(&h, side), ("Side 2".into(), 640, 360), "the dialog showed and set the right-clicked comp");
    assert_eq!(size(&h, main), ("Main".into(), 320, 180), "the open comp is untouched");
}
