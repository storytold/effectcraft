//! #3726: Render Queue multi-selection. Shift-click selects a range and Ctrl/⌘-click one more
//! item; Delete, the Render checkbox and the Render Settings menu then act on every selected item
//! as one undo step; Add to Render Queue queues every comp selected in the Project panel.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::project::render_queue::RenderQuality;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

/// Three comps, `Alpha`, `Beta` and `Gamma`, each queued, and the Render Queue shown.
fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    for name in ["Alpha", "Beta", "Gamma"] {
        let comp = s.execute("comp.new", json!({"name": name, "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
        s.execute("renderQueue.add", json!({"comp": comp})).unwrap();
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_context_setup(effectcraft_ui_egui::framework::install_headless)
        .build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::RenderQueue);
    h.run_steps(4);
    h
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str, modifiers: Modifiers) {
    let p = center(h, id);
    click_at(h, p, modifiers);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn keys_together(h: &mut Harness<'_, EffectcraftApp>, keys: &[Key]) {
    for pressed in [true, false] {
        for key in keys {
            h.input_mut().events.push(Event::Key { key: *key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        }
        h.step();
    }
    h.run_steps(3);
}
/// The comp names of the rows that say they are selected, in queue order.
fn selected(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    let mut rows: Vec<String> = h
        .query_all_by_role(Role::TreeItem)
        .filter(|n| n.accesskit_node().is_selected() == Some(true))
        .map(|n| n.accesskit_node().label().unwrap_or_default())
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    rows.sort();
    rows.iter().filter_map(|l| l.split_once(". ").and_then(|(_, rest)| rest.split_once(',')).map(|(name, _)| name.to_string())).collect()
}

fn queued(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    let s = &h.state().session;
    s.project.render_queue.iter().map(|i| s.project.item(i.comp).map(|c| c.name.clone()).unwrap_or_default()).collect()
}

#[test]
fn shift_and_ctrl_click_select_rows_and_delete_removes_them_all() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.row", Modifiers::NONE);
    assert_eq!(selected(&h), ["Alpha"]);
    click(&mut h, "renderQueue.item.3.row", Modifiers::SHIFT);
    assert_eq!(selected(&h), ["Alpha", "Beta", "Gamma"], "Shift-click selects the rows between");
    click(&mut h, "renderQueue.item.2.row", Modifiers::COMMAND);
    assert_eq!(selected(&h), ["Alpha", "Gamma"], "Ctrl/Cmd-click takes one row out");
    let steps = h.state().session.history.undo.len();
    key(&mut h, Key::Delete);
    assert_eq!(queued(&h), ["Beta"], "Delete removes every selected item");
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "as one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(queued(&h), ["Alpha", "Beta", "Gamma"]);
}

#[test]
fn arrow_down_then_delete_in_one_input_pass_removes_the_row_moved_to() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.row", Modifiers::NONE);
    keys_together(&mut h, &[Key::ArrowDown, Key::Delete]);
    assert_eq!(queued(&h), ["Alpha", "Gamma"], "Delete acts on the row the arrow moved to");
}
#[test]
fn an_arrow_at_the_anchor_collapses_a_range_before_delete() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.row", Modifiers::NONE);
    click(&mut h, "renderQueue.item.3.row", Modifiers::SHIFT);
    keys_together(&mut h, &[Key::ArrowUp, Key::Delete]);
    assert_eq!(queued(&h), ["Beta", "Gamma"], "only the row the arrow landed on is removed");
}
#[test]
fn the_render_checkbox_and_render_settings_reach_every_selected_item() {
    let mut h = harness();
    click(&mut h, "renderQueue.item.1.row", Modifiers::NONE);
    click(&mut h, "renderQueue.item.3.row", Modifiers::COMMAND);
    assert_eq!(selected(&h), ["Alpha", "Gamma"]);
    click(&mut h, "renderQueue.item.3.render", Modifiers::NONE);
    let on: Vec<bool> = h.state().session.project.render_queue.iter().map(|i| i.render).collect();
    assert_eq!(on, [false, true, false], "unticking one selected item unticks all of them");
    assert_eq!(selected(&h), ["Alpha", "Gamma"], "the checkbox leaves the selection alone");

    click(&mut h, "renderQueue.item.1.twirl", Modifiers::NONE);
    click(&mut h, "renderQueue.item.1.renderSettings", Modifiers::NONE);
    let entry = h.query_by_label_contains("Draft Settings").expect("the Draft Settings entry").rect();
    click_at(&mut h, entry.center(), Modifiers::NONE);
    let q: Vec<RenderQuality> = h.state().session.project.render_queue.iter().map(|i| i.settings.quality).collect();
    assert_eq!(q, [RenderQuality::Draft, RenderQuality::Best, RenderQuality::Draft], "the template reaches every selected item");
    let outputs: Vec<String> = h.state().session.project.render_queue.iter().filter_map(|i| h.state().session.resolve_output(i)).collect();
    assert!(outputs.iter().zip(["Alpha", "Beta", "Gamma"]).all(|(o, n)| o.ends_with(&format!("{n}.mp4"))), "names kept: {outputs:?}");
}

#[test]
fn add_to_render_queue_queues_every_comp_selected_in_the_project_panel() {
    let mut s = Session::default();
    let comps: Vec<u64> = ["One", "Two", "Three"]
        .iter()
        .map(|name| s.execute("comp.new", json!({"name": name, "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap())
        .collect();
    let mut app = EffectcraftApp::new(s);
    app.session.state.project_selection = comps.iter().map(|c| ItemId(*c)).collect();
    app.ui.focused = PanelKind::Project;
    let ctx = egui::Context::default();
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "renderQueue.add", json!({})).unwrap();
    let queued: Vec<u64> = app.session.project.render_queue.iter().map(|i| i.comp.0).collect();
    assert_eq!(queued, comps, "every selected comp, not only the active one");
    app.session.execute("edit.undo", json!({})).unwrap();
    assert!(app.session.project.render_queue.is_empty(), "one undo step");

    app.ui.focused = PanelKind::Timeline;
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "renderQueue.add", json!({})).unwrap();
    assert_eq!(app.session.project.render_queue.len(), 1, "from another panel, the active comp alone");
}
