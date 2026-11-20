//! Linked compositions use the existing Reload Footage menu entry and shortcut.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemId, ItemKind};
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::menus::parse_shortcut;
use egui::{Event, Key, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn pointer(h: &mut Harness<'_, EffectcraftApp>, id: &str, button: PointerButton) {
    let element = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    let p = pos2(element.rect[0] + element.rect[2] / 2.0, element.rect[1] + element.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

fn generation(h: &Harness<'_, EffectcraftApp>, item: ItemId) -> u64 {
    let ItemKind::Footage(f) = &h.state().session.project.item(item).unwrap().kind else { panic!("linked footage") };
    f.linked_comp.as_ref().unwrap().generation
}

#[test]
fn reload_footage_shortcut_refreshes_a_linked_source_without_reopening() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/ui-linked-reload");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Titles.ecproj");
    let mut source = Session::default();
    source.execute("comp.new", json!({"name":"Opening", "width":16, "height":16, "duration":1})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#ff0000"})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();

    let mut destination = Session::default();
    destination.execute("comp.new", json!({"name":"Main", "width":64, "height":64, "duration":1})).unwrap();
    let main = destination.active_comp_id().unwrap();
    let linked = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(), "comp":"Opening"})).unwrap();
    let linked = ItemId(linked["item"].as_u64().unwrap());
    let layer = destination.execute("layer.addItem", json!({"item":linked.0})).unwrap()["layer"].as_u64().unwrap();
    destination.execute("prop.set", json!({"layer":layer, "path":"transform/opacity", "value":75})).unwrap();
    let layers = destination.project.comp(main).unwrap().layers.clone();
    let before = destination.render(main, Tick::ZERO, Default::default());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(destination));
    h.run_steps(3);
    let row = format!("project.item.{}.name", linked.0);
    pointer(&mut h, &row, PointerButton::Primary);
    assert_eq!(h.state().ui.focused, PanelKind::Project);
    assert!(h.state().session.is_enabled("file.reloadFootage"));
    pointer(&mut h, &row, PointerButton::Secondary);
    let reload = h.query_by_label_contains("Reload Footage").expect("the existing Project context menu offers Reload Footage");
    assert!(!reload.accesskit_node().is_disabled(), "the linked source can be reloaded");
    key(&mut h, Key::Escape, Modifiers::NONE);

    source.execute("layer.newSolid", json!({"color":"#0000ff"})).unwrap();
    source.execute("file.save", json!({})).unwrap();
    let previous = generation(&h, linked);
    let (modifiers, command) = parse_shortcut("Cmd+Alt+L").unwrap();
    key(&mut h, command, modifiers);
    assert!(generation(&h, linked) > previous, "the shortcut reaches the linked loader");
    assert_eq!(h.state().session.project.comp(main).unwrap().layers, layers, "destination instance settings stay local");
    let after = h.state().session.render(main, Tick::ZERO, Default::default());
    assert_ne!(before.data, after.data, "the existing open destination shows the saved source edit");
    assert!(after.data.iter().any(|pixel| pixel[2] > pixel[0]));
}

#[test]
fn selected_link_exposes_its_source_and_offline_reason_to_keyboard_users() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/ui-linked-status");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Source.ecproj");
    let mut source = Session::default();
    source.execute("comp.new", json!({"name":"Opening","width":16,"height":16,"duration":1})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    let mut destination = Session::default();
    destination.execute("comp.new", json!({"name":"Main","width":64,"height":64,"duration":1})).unwrap();
    let linked = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":"Opening"})).unwrap();
    let linked = ItemId(linked["item"].as_u64().unwrap());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(destination));
    h.run_steps(3);
    let row_name = "Opening, Linked Composition";
    h.get_by_role_and_label(egui::accesskit::Role::TreeItem, row_name).focus();
    h.run_steps(3);
    let row = h.get_by_role_and_label(egui::accesskit::Role::TreeItem, row_name);
    assert!(row.is_focused());
    let description = row.accesskit_node().description().unwrap_or_default();
    assert!(description.contains("Source.ecproj") && description.contains("Opening") && description.contains("Reload Footage"));
    assert!(h.query_by_label_contains("Source project:").is_some(), "the source is visible in the selected-item details");
    let ready = h.get_by_role_and_label(egui::accesskit::Role::Status, "Ready — saved source snapshot");
    assert!(ready.rect().is_positive() && h.ctx.content_rect().contains_rect(ready.rect()));

    // Only this test's generated source is removed, then restored at the same path.
    std::fs::remove_file(&path).unwrap();
    assert!(h.state_mut().session.execute("file.reloadFootage", json!({"item":linked.0})).is_err());
    h.run_steps(3);
    let row = h.get_by_role_and_label(egui::accesskit::Role::TreeItem, row_name);
    assert!(row.accesskit_node().description().unwrap_or_default().contains("Offline:"));
    let status = h
        .query_all_by_role(egui::accesskit::Role::Status)
        .find(|node| node.accesskit_node().label().is_some_and(|label| label.starts_with("Offline:")))
        .unwrap();
    assert!(status.accesskit_node().description().unwrap_or_default().contains("Source.ecproj"));
    assert!(status.rect().is_positive() && h.ctx.content_rect().contains_rect(status.rect()));

    source.execute("file.save", json!({})).unwrap();
    h.state_mut().session.execute("file.reloadFootage", json!({"item":linked.0})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Status, "Ready — saved source snapshot").is_some());
    assert!(!h.get_by_role_and_label(egui::accesskit::Role::TreeItem, row_name).accesskit_node().description().unwrap_or_default().contains("Offline:"));
}
