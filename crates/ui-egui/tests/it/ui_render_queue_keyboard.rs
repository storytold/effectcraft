//! Render Queue keyboard operation and accessible names (docs/gaps.md gap 25): queue items are
//! tree items (named by number, comp and status; selected; open or closed); the arrow keys, Home /
//! End, Right / Left, Enter and Delete work them; the buttons, the check boxes, the twirls and the
//! drop-downs have names and states; a drop-down opens from the keyboard and closes with the focus
//! back on it; the Templates dialog is named.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    for name in ["Alpha", "Beta", "Gamma"] {
        let comp = s.execute("comp.new", json!({"name": name, "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
        s.execute("renderQueue.add", json!({"comp": comp})).unwrap();
    }
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::RenderQueue);
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

fn focused(h: &Harness<'_, EffectcraftApp>) -> Option<(Role, String)> {
    h.root().children_recursive().find(|n| n.is_focused()).map(|n| (n.accesskit_node().role(), n.accesskit_node().label().unwrap_or_default()))
}

fn queue_len(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.project.render_queue.len()
}

fn row_name(h: &Harness<'_, EffectcraftApp>, n: usize) -> String {
    h.query_all_by_role(Role::TreeItem).map(|r| r.accesskit_node().label().unwrap_or_default()).find(|l| l.starts_with(&format!("{n}. "))).unwrap_or_default()
}

#[test]
fn rows_are_named_tree_items_with_selected_and_open_state() {
    let mut h = harness();
    let first = row_name(&h, 1);
    assert!(first.starts_with("1. Alpha, "), "number, comp and status: {first}");
    let node = h.get_by_role_and_label(Role::TreeItem, &first);
    assert_eq!(node.accesskit_node().is_selected(), Some(false));
    assert_eq!(node.accesskit_node().data().is_expanded(), Some(false));
    assert_eq!(node.accesskit_node().data().level(), Some(1));
    click(&mut h, "renderQueue.item.1.twirl");
    let node = h.get_by_role_and_label(Role::TreeItem, &first);
    assert_eq!(node.accesskit_node().data().is_expanded(), Some(true), "the open item says so");
    let twirl = h.get_by_label("Hide details of Alpha");
    assert_eq!(twirl.accesskit_node().role(), Role::Button);
    assert_eq!(twirl.accesskit_node().data().is_expanded(), Some(true));
    // The settings drop-downs of the open item: what they set and what they are set to.
    let settings = h.get_by_role_and_label(Role::ComboBox, "Render Settings");
    assert!(settings.accesskit_node().value().is_some_and(|v| !v.is_empty()));
    assert!(h.query_by_role_and_label(Role::ComboBox, "Output Module").is_some());
    assert!(h.query_by_role_and_label(Role::ComboBox, "Log").is_some());
    assert!(h.query_by_role_and_label(Role::ComboBox, "Output To").is_some());
    assert!(h.query_by_role_and_label(Role::Button, "Render").is_some());
    let notify = h.get_by_role_and_label(Role::CheckBox, "Notify");
    assert!(notify.accesskit_node().toggled().is_some());
    let render = h.get_by_role_and_label(Role::CheckBox, "Render: Alpha");
    assert_eq!(render.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert_eq!(h.get_by_role(Role::ProgressIndicator).accesskit_node().data().numeric_value(), Some(0.0));
}

#[test]
fn arrows_home_end_right_left_enter_and_delete_work_the_rows() {
    let mut h = harness();
    h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).focus();
    h.run_steps(3);
    assert_eq!(focused(&h).map(|f| f.1), Some(row_name(&h, 1)));
    let selected = |h: &Harness<'_, EffectcraftApp>| {
        h.query_all_by_role(Role::TreeItem)
            .filter(|n| n.accesskit_node().is_selected() == Some(true))
            .map(|n| n.accesskit_node().label().unwrap_or_default())
            .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
            .collect::<Vec<_>>()
    };
    assert_eq!(selected(&h), vec![row_name(&h, 1)], "a focused row is selected");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(row_name(&h, 2)), "Down Arrow moves to the next item and the focus goes with it");
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 2)).accesskit_node().is_selected(), Some(true));
    key(&mut h, Key::End, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(row_name(&h, 3)));
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(row_name(&h, 3)), "Down Arrow stays on the last item");
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(row_name(&h, 1)));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).accesskit_node().data().is_expanded(), Some(true), "Right Arrow opens the item");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).accesskit_node().data().is_expanded(), Some(false), "Left Arrow closes it");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).accesskit_node().data().is_expanded(), Some(true), "Enter toggles it");
    key(&mut h, Key::Space, Modifiers::NONE);
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).accesskit_node().data().is_expanded(), Some(false), "Space toggles it back");
    assert!(!h.state().playback.playing && !h.state().playback.caching && h.state().playback.plan.is_none(), "Space on a row does not start a preview");
    // Delete removes the selected item, as it does with the mouse's selection.
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(queue_len(&h), 3);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(queue_len(&h), 2, "Delete removes the selected item");
}

#[test]
fn tab_reaches_every_control_of_a_row() {
    let mut h = harness();
    h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).focus();
    h.run_steps(3);
    let mut seen = vec![focused(&h).unwrap()];
    for _ in 0..40 {
        key(&mut h, Key::Tab, Modifiers::NONE);
        match focused(&h) {
            Some(f) if !seen.contains(&f) => seen.push(f),
            _ => {}
        }
    }
    let names: Vec<&str> = seen.iter().map(|s| s.1.as_str()).collect();
    for want in ["Show details of Alpha", "Render: Alpha", "Show details of Beta", "Render: Beta"] {
        assert!(names.contains(&want), "Tab reaches `{want}`; reached {names:?}");
    }
}

#[test]
fn a_drop_down_opens_from_the_keyboard_and_gives_the_focus_back() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.twirl");
    h.get_by_role_and_label(Role::ComboBox, "Render Settings").focus();
    h.run_steps(3);
    key(&mut h, Key::Enter, Modifiers::NONE);
    let (role, first_entry) = focused(&h).expect("the first entry has the focus");
    assert_ne!(role, Role::ComboBox, "the keyboard is in the menu: {first_entry}");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let second = focused(&h).expect("Down Arrow moves to the next entry").1;
    assert_ne!(second, first_entry);
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(first_entry), "Up Arrow moves back");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::ComboBox), "choosing an entry gives the focus back to the drop-down");
}

#[test]
fn the_check_box_and_the_buttons_work_from_the_keyboard() {
    let mut h = harness();
    let before = h.state().session.project.render_queue.first().map(|i| i.render);
    h.get_by_role_and_label(Role::CheckBox, "Render: Alpha").focus();
    h.run_steps(3);
    key(&mut h, Key::Space, Modifiers::NONE);
    assert_ne!(h.state().session.project.render_queue.first().map(|i| i.render), before, "Space toggles the Render check box");
    let notify = h.state().session.project.render_prefs.notify;
    h.get_by_role_and_label(Role::CheckBox, "Notify").focus();
    h.run_steps(3);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_ne!(h.state().session.project.render_prefs.notify, notify, "Enter toggles Notify");
}

#[test]
fn the_templates_dialog_is_named() {
    let mut h = harness();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.templates", json!({"kind": "renderSettings"})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_role_and_label(Role::ComboBox, "Settings Name").is_some());
    assert!(h.query_by_role_and_label(Role::TextInput, "template name").is_some());
    for name in ["Duplicate", "Delete", "   OK   "] {
        assert_eq!(h.get_by_role_and_label(Role::Button, name).accesskit_node().role(), Role::Button, "{name}");
    }
    let defaults = h.query_all_by_role(Role::ComboBox).count();
    assert!(defaults >= 4, "the default slots are drop-downs: {defaults}");
}

/// A pick with the mouse leaves no widget with the keyboard focus: egui pauses the application's
/// shortcuts (Space, Ctrl+Z) while one has it (#776's rule for the Project panel).
#[test]
fn a_pick_with_the_mouse_leaves_no_keyboard_focus() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.twirl");
    click(&mut h, "renderQueue.item.1.renderSettings");
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "opening a drop-down with the mouse takes no keyboard focus");
    let template = h.state().session.project.render_templates.render_settings_list()[0].name.clone();
    h.get_by_label_contains(&template).click();
    h.run_steps(4);
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "choosing an entry with the mouse leaves no keyboard focus");
    // The Effects & Presets options menu the same.
}

/// Enter after a click in the panel belongs to the panel (it opens or applies the row), not to
/// the layer rename the Timeline binds to it.
#[test]
fn enter_after_a_click_opens_the_item_and_does_not_rename_a_layer() {
    let mut h = harness();
    h.state_mut().session.execute("layer.newSolid", json!({"name": "Red", "color": "#c04040"})).unwrap();
    h.run_steps(3);
    click(&mut h, "renderQueue.item.1.row");
    assert_eq!(h.state().ui.focused, PanelKind::RenderQueue);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(h.get_by_role_and_label(Role::TreeItem, &row_name(&h, 1)).accesskit_node().data().is_expanded(), Some(true), "Enter opens the clicked item");
    assert!(h.state().auto.query("timeline.layer.").iter().all(|e| !e.id.ends_with(".nameEdit")), "no layer rename started");
}
