//! Effects & Presets keyboard operation and accessible names (docs/gaps.md gap 25): the list is one
//! Tab stop; the arrow keys, Home / End, Right / Left, Enter and Space work it; the focus follows
//! the cursor; rows are tree items with a name, a selected state, a depth and (folders) an open
//! state; the favourite star is a named button.

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
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#c04040"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::EffectsPresets);
    h.run_steps(4);
    h
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

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.run_steps(3);
}

/// The role and name of the node that has the keyboard focus.
fn focused(h: &Harness<'_, EffectcraftApp>) -> Option<(Role, String)> {
    h.root().children_recursive().find(|n| n.is_focused()).map(|n| (n.accesskit_node().role(), n.accesskit_node().label().unwrap_or_default()))
}

fn selected_rows(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    let panel = h.state().auto.find("panel.EffectsPresets").expect("the panel is drawn").rect;
    let panel = egui::Rect::from_min_size(pos2(panel[0], panel[1]), egui::vec2(panel[2], panel[3]));
    h.root()
        .children_recursive()
        .filter(|n| n.accesskit_node().role() == Role::TreeItem && n.accesskit_node().is_selected() == Some(true))
        .filter(|n| n.accesskit_node().bounding_box().is_some_and(|b| panel.contains(pos2(b.x0 as f32 + 1.0, b.y0 as f32 + 1.0))))
        .map(|n| n.accesskit_node().label().unwrap_or_default())
        .collect()
}

/// Put the keyboard on the first row the way Tab does: from the search field, the menu button,
/// then the list.
fn tab_into_list(h: &mut Harness<'_, EffectcraftApp>) {
    h.get_by_label("Search effects and presets").focus();
    h.run_steps(3);
    key(h, Key::Tab, Modifiers::NONE);
    assert_eq!(focused(h).map(|f| f.1), Some("Effects & Presets options".to_string()), "Tab leaves the search field for the options button");
    key(h, Key::Tab, Modifiers::NONE);
}

#[test]
fn tab_reaches_the_list_once_and_leaves_it() {
    let mut h = harness();
    tab_into_list(&mut h);
    let (role, name) = focused(&h).expect("a row has the focus");
    assert_eq!(role, Role::TreeItem);
    assert!(!name.is_empty());
    // The first row is a folder: no star. The next Tab leaves the list instead of walking the
    // hundreds of effects.
    key(&mut h, Key::Tab, Modifiers::NONE);
    let after = focused(&h);
    assert!(after.as_ref().is_none_or(|(r, _)| *r != Role::TreeItem), "the second Tab is past the list: {after:?}");
}

#[test]
fn arrows_home_end_move_the_cursor_and_the_focus() {
    let mut h = harness();
    tab_into_list(&mut h);
    let first = focused(&h).unwrap().1;
    assert_eq!(selected_rows(&h), Vec::<String>::new(), "Tab alone selects nothing");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let second = focused(&h).unwrap().1;
    assert_ne!(first, second, "Down Arrow moves to the next row");
    assert_eq!(selected_rows(&h), vec![second.clone()], "the row the cursor is on is selected");
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focused(&h).unwrap().1, first);
    key(&mut h, Key::End, Modifiers::NONE);
    let last = focused(&h).unwrap().1;
    assert_ne!(last, first, "End goes to the last row");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h).unwrap().1, last, "Down Arrow stays on the last row");
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(focused(&h).unwrap().1, first, "Home goes to the first row");
    key(&mut h, Key::PageDown, Modifiers::NONE);
    assert_ne!(focused(&h).unwrap().1, first, "Page Down moves a page");
    // The cursor row is scrolled into view: its node is on screen.
    let name = focused(&h).unwrap().1;
    let r = h.get_by_label(&name).rect();
    assert!(r.min.y >= 0.0 && r.max.y <= 1000.0, "the focused row is on screen: {r:?}");
}

#[test]
fn right_and_left_open_and_close_folders_and_step_through_the_tree() {
    let mut h = harness();
    h.state_mut().ui.effects_search.clear();
    tab_into_list(&mut h);
    // Walk to a category folder (not the presets folder).
    let mut guard = 0;
    loop {
        let n = focused(&h).unwrap().1;
        if n == "Blur & Sharpen" {
            break;
        }
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
        guard += 1;
        assert!(guard < 40, "no Blur & Sharpen folder reached");
    }
    let folder = h.get_by_label("Blur & Sharpen");
    assert_eq!(folder.accesskit_node().data().is_expanded(), Some(false));
    assert_eq!(folder.accesskit_node().data().level(), Some(1));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert!(h.state().ui.effects_open.contains("Blur & Sharpen"), "Right Arrow opens the folder");
    assert_eq!(h.get_by_label("Blur & Sharpen").accesskit_node().data().is_expanded(), Some(true));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    let child = focused(&h).unwrap();
    assert_eq!(child.0, Role::TreeItem);
    assert_ne!(child.1, "Blur & Sharpen", "Right Arrow on an open folder steps into it");
    assert_eq!(h.get_by_label(&child.1).accesskit_node().data().level(), Some(2));
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focused(&h).unwrap().1, "Blur & Sharpen", "Left Arrow on an effect steps out to its folder");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert!(!h.state().ui.effects_open.contains("Blur & Sharpen"), "Left Arrow closes the folder");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().ui.effects_open.contains("Blur & Sharpen"), "Enter opens a closed folder");
    key(&mut h, Key::Space, Modifiers::NONE);
    assert!(!h.state().ui.effects_open.contains("Blur & Sharpen"), "Space closes it again");
}

#[test]
fn enter_and_space_apply_an_effect_to_the_selected_layer() {
    for apply_key in [Key::Enter, Key::Space] {
        let mut h = harness();
        tab_into_list(&mut h);
        h.state_mut().ui.effects_open.insert("Blur & Sharpen".into());
        h.run_steps(3);
        let mut guard = 0;
        while focused(&h).map(|f| f.1) != Some("Blur & Sharpen".to_string()) {
            key(&mut h, Key::ArrowDown, Modifiers::NONE);
            guard += 1;
            assert!(guard < 40);
        }
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
        let effect = focused(&h).unwrap().1;
        assert!(h.state().session.state.last_effect.is_none());
        key(&mut h, apply_key, Modifiers::NONE);
        let applied = h.state().session.state.last_effect.clone();
        assert!(applied.is_some(), "{apply_key:?} on `{effect}` applies it to the selected layer");
        let pb = &h.state().playback;
        assert!(!pb.playing && !pb.caching && pb.plan.is_none(), "{apply_key:?} on a focused row is the row's, it does not start a preview");
    }
}

#[test]
fn the_star_is_a_named_button_on_the_cursor_row_and_enter_toggles_it() {
    let mut h = harness();
    tab_into_list(&mut h);
    h.state_mut().ui.effects_open.insert("Blur & Sharpen".into());
    h.run_steps(3);
    while focused(&h).map(|f| f.1) != Some("Blur & Sharpen".to_string()) {
        key(&mut h, Key::ArrowDown, Modifiers::NONE);
    }
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let effect = focused(&h).unwrap().1;
    key(&mut h, Key::Tab, Modifiers::NONE);
    let (role, name) = focused(&h).expect("the star has the focus");
    assert_eq!(role, Role::Button);
    assert_eq!(name, format!("Add to favorites: {effect}"));
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(h.state().session.prefs.effects_favorites.len(), 1, "Enter on the star adds the favourite");
    let (_, name) = focused(&h).expect("the star keeps the focus");
    assert_eq!(name, format!("Remove from favorites: {effect}"));
}

#[test]
fn the_search_field_keeps_its_arrows_and_the_layer_does_not_nudge() {
    let mut h = harness();
    h.get_by_label("Search effects and presets").focus();
    h.run_steps(3);
    h.get_by_label("Search effects and presets").type_text("blur");
    h.run_steps(3);
    assert_eq!(h.state().ui.effects_search, "blur");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert!(selected_rows(&h).is_empty(), "arrows in the search field belong to the field");
    // A click on a row, then the arrows: the list moves, the selected layer stays where it is.
    let layer_before = layer_position(&h);
    h.state_mut().ui.effects_search.clear();
    h.run_steps(3);
    click(&mut h, "effects.category.presets");
    assert_eq!(h.state().ui.focused, PanelKind::EffectsPresets);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(selected_rows(&h).len(), 1, "the arrows work after a click, without keyboard focus on a row");
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(layer_position(&h), layer_before, "the arrow keys did not nudge the selected layer");
}

fn layer_position(h: &Harness<'_, EffectcraftApp>) -> String {
    let s = &h.state().session;
    let comp = s.active_comp().expect("a comp");
    let layer = comp.layers.first().expect("a layer");
    format!("{:?}", layer.transform())
}

#[test]
fn rows_and_controls_have_names_roles_and_states() {
    let mut h = harness();
    h.state_mut().ui.effects_open.insert("Blur & Sharpen".into());
    h.state_mut().session.prefs.toggle_effect_favorite("ec.blur.gaussian");
    h.run_steps(4);
    assert_eq!(h.get_by_label("Search effects and presets").accesskit_node().role(), Role::TextInput);
    let menu = h.get_by_label("Effects & Presets options");
    assert_eq!(menu.accesskit_node().role(), Role::Button);
    assert_eq!(menu.accesskit_node().data().is_expanded(), Some(false), "the options button says its menu is closed");
    let folder = h.get_by_label("Blur & Sharpen");
    assert_eq!(folder.accesskit_node().role(), Role::TreeItem);
    assert_eq!(folder.accesskit_node().data().is_expanded(), Some(true));
    assert_eq!(folder.accesskit_node().is_selected(), Some(false));
    let favorites = h.get_by_label("Favorites");
    assert_eq!(favorites.accesskit_node().data().is_expanded(), Some(false));
    // Every effect row in an open category is a leaf at depth 2.
    let leaves: Vec<_> =
        h.root().children_recursive().filter(|n| n.accesskit_node().role() == Role::TreeItem && n.accesskit_node().data().level() == Some(2)).collect();
    assert!(!leaves.is_empty());
    assert!(leaves.iter().all(|n| n.accesskit_node().data().is_expanded().is_none()), "effects are leaves: they have no open state");
    assert!(h.query_by_label_contains("Remove from favorites: ").is_none(), "a favourite hidden in a closed Favorites folder draws no rows");
    assert!(h.query_all_by_label_contains("Add to favorites: ").count() > 1, "the effects' stars are named");
}

/// A pick from the options menu with the mouse leaves no widget with the keyboard focus, and Enter
/// after a click in the panel is the panel's: it opens or applies the row and does not start the
/// layer rename the Timeline binds to Enter.
#[test]
fn mouse_picks_leave_no_focus_and_enter_after_a_click_is_the_panels() {
    let mut h = harness();
    click(&mut h, "effects.menu");
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "opening the menu with the mouse takes no keyboard focus");
    h.get_by_label_contains("Alphabetical").click();
    h.run_steps(4);
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "choosing an entry with the mouse leaves no keyboard focus");
    assert!(h.state().ui.effects_view.alphabetical);
    h.state_mut().ui.effects_view.alphabetical = false;
    h.run_steps(3);
    click(&mut h, "effects.category.presets");
    let open = h.state().ui.effects_open.contains("presets");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_ne!(h.state().ui.effects_open.contains("presets"), open, "Enter toggles the clicked folder");
    assert!(h.state().auto.query("timeline.layer.").iter().all(|e| !e.id.ends_with(".nameEdit")), "no layer rename started");
}
