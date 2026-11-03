//! Settings keyboard operation and accessible names (docs/gaps.md gap 25): the page list is one
//! Tab stop worked with the arrow keys, Home and End; every control on a page has the name its
//! label shows, with its value; Escape still cancels.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::accesskit::{Role, Toggled};
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness(page: &str) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": page})).unwrap();
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

fn focused(h: &Harness<'_, EffectcraftApp>) -> Option<(Role, String)> {
    h.root().children_recursive().find(|n| n.is_focused()).map(|n| (n.accesskit_node().role(), n.accesskit_node().label().unwrap_or_default()))
}

fn titles() -> Vec<&'static str> {
    effectcraft_engine::prefs::pages().iter().map(|p| p.title).collect()
}

#[test]
fn the_page_list_is_one_tab_stop_worked_with_the_arrows() {
    let mut h = harness("general");
    let titles = titles();
    let n = titles.len();
    // Every page is a list item: its place in the list, and the open page selected.
    for (i, title) in titles.iter().enumerate() {
        let item = h.get_by_role_and_label(Role::ListItem, title);
        assert_eq!(item.accesskit_node().role(), Role::ListItem, "{title}");
        assert_eq!(item.accesskit_node().data().position_in_set(), Some(i + 1));
        assert_eq!(item.accesskit_node().data().size_of_set(), Some(n));
        assert_eq!(item.accesskit_node().is_selected(), Some(i == 0), "{title}");
    }
    h.get_by_role_and_label(Role::ListItem, titles[0]).focus();
    h.run_steps(3);
    assert_eq!(focused(&h), Some((Role::ListItem, titles[0].to_string())));
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h), Some((Role::ListItem, titles[1].to_string())), "Down Arrow opens the next page and the focus goes with it");
    assert_eq!(h.get_by_role_and_label(Role::ListItem, titles[1]).accesskit_node().is_selected(), Some(true));
    assert_eq!(h.get_by_role_and_label(Role::ListItem, titles[0]).accesskit_node().is_selected(), Some(false));
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(titles[0].to_string()));
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(titles[0].to_string()), "Up Arrow stops at the first page");
    key(&mut h, Key::End, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(titles[n - 1].to_string()), "End goes to the last page");
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(titles[0].to_string()), "Home goes to the first page");
    // Only the open page is a Tab stop: Tab leaves the list for the page's first control.
    key(&mut h, Key::Tab, Modifiers::NONE);
    let (role, _) = focused(&h).expect("Tab moves the focus on");
    assert_ne!(role, Role::ListItem, "Tab leaves the page list");
}

#[test]
fn controls_have_the_name_their_label_shows_and_their_value() {
    let h = harness("general");
    let undo = h.get_by_label("Levels of Undo");
    assert_eq!(undo.accesskit_node().role(), Role::SpinButton);
    assert!(undo.accesskit_node().numeric_value().is_some(), "a spin button carries its number");
    let language = h.get_by_label("Language");
    assert_eq!(language.accesskit_node().role(), Role::ComboBox);
    assert!(language.accesskit_node().value().is_some_and(|v| !v.is_empty()), "a drop-down carries what it is set to");
    let tips = h.get_by_label("Show Tool Tips");
    assert_eq!(tips.accesskit_node().role(), Role::CheckBox);
    assert!(matches!(tips.accesskit_node().toggled(), Some(Toggled::True | Toggled::False)));
    for name in ["   OK   ", "Cancel", "Previous", "Next"] {
        assert_eq!(h.get_by_label(name).accesskit_node().role(), Role::Button, "{name}");
    }
    // Previous is disabled on the first page.
    assert!(h.get_by_label("Previous").accesskit_node().is_disabled());
}

#[test]
fn text_fields_colour_buttons_and_theme_cards_are_named() {
    let h = harness("labels");
    let name = h.get_by_label("Label 1 name");
    assert_eq!(name.accesskit_node().role(), Role::TextInput);
    assert!(name.accesskit_node().value().is_some_and(|v| !v.is_empty()), "the field carries its text");
    let color = h.get_by_label("Label 1 color");
    assert_eq!(color.accesskit_node().role(), Role::ColorWell);
    assert!(color.accesskit_node().value().is_some_and(|v| v.starts_with('#')), "a colour carries its hex value");
    let h = harness("appearance");
    assert_eq!(h.get_by_label("Appearance Mode").accesskit_node().role(), Role::ComboBox);
    assert!(h.query_all_by_role(Role::RadioButton).count() >= 2, "the themes are radio buttons");
}

#[test]
fn escape_still_cancels_with_the_keyboard_on_the_page_list() {
    let mut h = harness("general");
    let first = titles()[0];
    h.get_by_role_and_label(Role::ListItem, first).focus();
    h.run_steps(3);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().dialog.is_none(), "Escape closes the dialog");
}
