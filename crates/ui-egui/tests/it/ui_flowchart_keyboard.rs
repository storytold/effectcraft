//! Flowchart keyboard operation and accessible names (docs/gaps.md gap 25): the nodes are list
//! items ("Mid, Composition, uses 1": selected, place in the chart), the chart is one Tab stop,
//! the arrow keys, Home / End, Enter and Space work the nodes, and the toolbar switches say whether
//! they are on.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::{Role, Toggled};
use egui::{Event, Key, Modifiers, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

/// Main uses Mid and Inner, Mid uses Inner: three comp nodes (Main, Mid, Inner) and a solid.
fn harness() -> Harness<'static, EffectcraftApp> {
    sized(1600.0, 1000.0)
}

fn sized(w: f32, hh: f32) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    let id = |v: serde_json::Value| v["id"].as_u64().or_else(|| v["comp"].as_u64()).or_else(|| v["item"].as_u64()).unwrap();
    let inner = id(s.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "duration": 2})).unwrap());
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000", "width": 100, "height": 100})).unwrap();
    s.execute("layer.select", json!({"layers": []})).unwrap();
    let mid = id(s.execute("comp.new", json!({"name": "Mid", "width": 100, "height": 100, "duration": 2})).unwrap());
    s.execute("layer.addItem", json!({"item": inner})).unwrap();
    id(s.execute("comp.new", json!({"name": "Main", "width": 100, "height": 100, "duration": 2})).unwrap());
    s.execute("layer.addItem", json!({"item": mid})).unwrap();
    s.execute("layer.addItem", json!({"item": inner})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(w, hh)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Flowchart);
    h.state_mut().ui.maximized = Some(PanelKind::Flowchart);
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

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
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

fn names(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.query_all_by_role(Role::ListItem).map(|n| n.accesskit_node().label().unwrap_or_default()).collect()
}

fn selected(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.query_all_by_role(Role::ListItem)
        .filter(|n| n.accesskit_node().is_selected() == Some(true))
        .map(|n| n.accesskit_node().label().unwrap_or_default())
        .collect()
}

fn focus_node(h: &mut Harness<'_, EffectcraftApp>, name: &str) {
    h.get_by_role_and_label(Role::ListItem, name).focus();
    h.run_steps(3);
}

#[test]
fn nodes_are_named_list_items_with_what_they_are_and_what_they_use() {
    let h = harness();
    let all = names(&h);
    for want in ["Main, Composition, uses 2", "Mid, Composition, uses 1", "Inner, Composition, uses 1", "Red, Solid"] {
        assert!(all.iter().any(|n| n == want), "`{want}` among {all:?}");
    }
    let node = h.get_by_role_and_label(Role::ListItem, "Mid, Composition, uses 1");
    assert_eq!(node.accesskit_node().is_selected(), Some(false));
    assert_eq!(node.accesskit_node().data().size_of_set(), Some(all.len()));
    assert!(node.accesskit_node().data().position_in_set().is_some_and(|p| (1..=all.len()).contains(&p)));
    assert_eq!(h.get_by_role_and_label(Role::Group, "Flowchart: Main").accesskit_node().role(), Role::Group, "the chart has a name");
}

#[test]
fn the_toolbar_switches_say_whether_they_are_on() {
    let mut h = harness();
    let state = |h: &Harness<'_, EffectcraftApp>, name: &str| h.get_by_role_and_label(Role::Button, name).accesskit_node().toggled();
    assert_eq!(state(&h, "Solids"), Some(Toggled::True));
    assert_eq!(state(&h, "Layers"), Some(Toggled::False));
    h.get_by_role_and_label(Role::Button, "Layers").focus();
    h.run_steps(3);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(state(&h, "Layers"), Some(Toggled::True), "Enter on the focused switch turns it on");
    assert!(h.state().ui.flowchart.layers);
    assert!(h.query_by_role_and_label(Role::Button, "Left to Right").is_some(), "the direction button is named");
}

#[test]
fn the_chart_is_one_tab_stop_and_the_arrows_move_between_the_nodes() {
    let mut h = harness();
    // Tab from the toolbar passes through exactly one node.
    h.get_by_role_and_label(Role::Button, "Solids").focus();
    h.run_steps(3);
    let mut nodes_seen = std::collections::BTreeSet::new();
    for _ in 0..12 {
        key(&mut h, Key::Tab, Modifiers::NONE);
        if let Some((Role::ListItem, name)) = focused(&h) {
            nodes_seen.insert(name);
        }
    }
    assert_eq!(nodes_seen.len(), 1, "one Tab stop for the chart: {nodes_seen:?}");

    let all = names(&h);
    focus_node(&mut h, &all[0]);
    assert_eq!(selected(&h), vec![all[0].clone()], "a focused node is selected");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(all[1].clone()), "Down Arrow moves to the next node and the focus goes with it");
    assert_eq!(selected(&h), vec![all[1].clone()]);
    key(&mut h, Key::End, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), all.last().cloned());
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), all.last().cloned(), "Down Arrow stays on the last node");
    key(&mut h, Key::Home, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(all[0].clone()));
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some(all[0].clone()), "Up Arrow stays on the first node");
}

#[test]
fn right_goes_to_what_a_node_uses_left_goes_back_and_enter_opens_the_comp() {
    let mut h = harness();
    // The chart's Tab stop is its selected node, the first one with none selected.
    focus_node(&mut h, "Main, Composition, uses 2");
    assert_eq!(focused(&h).map(|f| f.1), Some("Main, Composition, uses 2".to_string()));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    let child = focused(&h).expect("Right Arrow moves to a node Main uses").1;
    assert!(child.starts_with("Mid, ") || child.starts_with("Inner, "), "{child}");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some("Main, Composition, uses 2".to_string()), "Left Arrow goes back to the node that uses it");
    // (Right Arrow from Main goes to the first node it uses: the top layer, Inner.)
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(focused(&h).map(|f| f.1), Some("Inner, Composition, uses 1".to_string()));
    key(&mut h, Key::Enter, Modifiers::NONE);
    let status = h.state().ui.status.clone();
    let active = h.state().session.active_comp_id().and_then(|id| h.state().session.project.item(id)).map(|i| i.name.clone());
    assert_eq!(active.as_deref(), Some("Inner"), "Enter opens the comp, as a double click does (status: {status})");
}

#[test]
fn after_a_click_the_arrows_belong_to_the_chart_and_do_not_nudge_layers() {
    let mut h = harness();
    // Select the Mid layer in Main, then click a node: the arrows must not move the layer.
    h.state_mut().session.execute("layer.select", json!({"layers": ["#1"]})).ok();
    h.run_steps(3);
    let before = serde_json::to_string(&h.state().session.project).unwrap_or_default();
    let node = h.state().auto.find("flowchart.node.item:3").or_else(|| h.state().auto.query("flowchart.node.").into_iter().next()).cloned().expect("a node");
    click_at(&mut h, pos2(node.rect[0] + node.rect[2] / 2.0, node.rect[1] + node.rect[3] / 2.0));
    assert_eq!(h.state().ui.focused, PanelKind::Flowchart);
    let selected_before = selected(&h);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_ne!(selected(&h), selected_before, "the arrow moved the chart's selection");
    assert_eq!(serde_json::to_string(&h.state().session.project).unwrap_or_default(), before, "no layer was nudged");
}

#[test]
fn a_node_scrolled_out_of_view_is_brought_back_by_the_keys() {
    // A small window: the last node starts out of view (it is not in the tree until the key
    // brings it in).
    let mut h = sized(300.0, 400.0);
    let all = names(&h);
    let ctx = h.ctx.clone();
    let graph = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "flowchart.graph", json!({})).unwrap();
    let total = graph["nodes"].as_array().map_or(0, Vec::len);
    assert!(all.len() < total, "the window is small enough to hide a node ({} of {total} drawn)", all.len());
    focus_node(&mut h, &all[0]);
    key(&mut h, Key::End, Modifiers::NONE);
    let last_id = graph["nodes"].as_array().and_then(|n| n.last()).and_then(|n| n["id"].as_str()).unwrap_or_default().to_string();
    assert_eq!(h.state().ui.flowchart.selected.as_deref(), Some(last_id.as_str()), "End selects the last node");
    assert_eq!(
        focused(&h).map(|f| f.0),
        Some(Role::ListItem),
        "the last node is drawn (the view panned to it) and has the focus; pan {:?}",
        h.state().ui.flowchart.pan
    );
}
