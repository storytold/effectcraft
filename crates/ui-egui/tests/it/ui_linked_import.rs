//! A source-comp picker links saved compositions without duplicating existing project links.

use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_engine::project::ItemKind;
use effectcraft_ui_egui::{Dialog, DialogDone, EffectcraftApp, FileDialog, menus};
use egui::accesskit::{Role, Toggled};
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn source(directory: &str) -> (Session, std::path::PathBuf) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Source.ecproj");
    let mut source = Session::default();
    for name in ["Existing", "Opening", "Credits"] {
        source.execute("comp.new", json!({"name":name,"width":32,"height":24,"duration":2})).unwrap();
    }
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    (source, path)
}

fn harness(path: &std::path::Path) -> Harness<'static, EffectcraftApp> {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Main","width":64,"height":64,"duration":2})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    let path = path.to_string_lossy().to_string();
    h.state_mut().hooks.pick_files = Some(Box::new(move |extensions| {
        assert_eq!(extensions, &["ecproj"]);
        vec![path.clone()]
    }));
    h
}

fn open(h: &mut Harness<'_, EffectcraftApp>) {
    let context = h.ctx.clone();
    menus::invoke(h.state_mut(), &context, "file.linkComposition", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::LinkedImport));
}

fn select(h: &mut Harness<'_, EffectcraftApp>, name: &str) {
    h.get_by_role_and_label(Role::CheckBox, name).focus();
    h.run_steps(2);
    key(h, Key::Space);
    assert_eq!(h.get_by_role_and_label(Role::CheckBox, name).accesskit_node().toggled(), Some(Toggled::True));
}

fn link_selected(h: &mut Harness<'_, EffectcraftApp>) {
    h.get_by_role_and_label(Role::Button, "Link Selected").focus();
    h.run_steps(2);
    key(h, Key::Enter);
}

#[test]
fn source_picker_tags_existing_links_and_links_two_new_comps_as_one_undo() {
    let (_source, path) = source("linked-import-ui-workflow");
    let mut h = harness(&path);
    h.state_mut().session.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":"Existing"})).unwrap();
    h.run_steps(3);
    let before = h.state().session.project.items.len();
    let undo = h.state().session.history.undo.len();
    let entry = menus::menu_items(h.state()).into_iter().find(|entry| entry.id == "file.linkComposition").expect("link importer is in the menu");
    assert_eq!(entry.path, ["File", "Import"]);
    open(&mut h);
    assert_eq!(h.state().session.project.items.len(), before, "opening the source list does not import it");
    let existing = h.get_by_role_and_label(Role::CheckBox, "Existing — Already linked");
    assert!(existing.accesskit_node().is_disabled());
    assert_eq!(existing.accesskit_node().toggled(), Some(Toggled::True));
    assert!(h.get_by_role_and_label(Role::Button, "Link Selected").accesskit_node().is_disabled());
    let opening = h.get_by_role_and_label(Role::CheckBox, "Opening").rect();
    let choose = h.get_by_role_and_label(Role::Button, "Choose Project…").rect();
    let reload = h.get_by_role_and_label(Role::Button, "Reload List").rect();
    let link = h.get_by_role_and_label(Role::Button, "Link Selected").rect();
    let source_heading = h.get_by_label("Source").rect();
    assert!(h.query_by_label("Source project").is_none(), "the project name replaces the redundant heading");
    assert!(opening.is_positive() && h.ctx.content_rect().contains_rect(opening));
    assert!(choose.intersects(egui::Rect::from_x_y_ranges(choose.x_range(), source_heading.y_range())), "Choose Project sits beside its source heading");
    assert!((choose.left() - reload.left()).abs() < 1.0 && choose.left() <= opening.left(), "header buttons align with the list's left edge");
    assert!(choose.bottom() < reload.top() && reload.bottom() < opening.top(), "source and reload controls sit above the composition rows");
    assert!(opening.bottom() < link.top(), "the source list is visibly above its footer: {opening:?}, {link:?}");
    assert!(
        h.query_all_by_label_contains("fps").any(|node| {
            let stats = node.rect();
            stats.left() >= opening.right() && stats.center().y >= opening.top() && stats.center().y <= opening.bottom()
        }),
        "composition stats sit to the right on the same line as their checkbox"
    );
    assert!(
        h.root().children_recursive().any(|node| node.is_focused() && node.accesskit_node().role() == Role::CheckBox),
        "the picker takes real keyboard focus"
    );

    if let Some(directory) = std::env::var_os("LINKED_IMPORT_SNAPSHOT") {
        use crate::gpu_test::RenderSerial;
        use effectcraft_ui_egui::theme::ThemeKind;
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        for (kind, brightness, name) in [
            (ThemeKind::Dark, 0.0, "dark"),
            (ThemeKind::Light, 0.0, "light"),
            (ThemeKind::Dark, -1.0, "dark-low"),
            (ThemeKind::Dark, 1.0, "dark-high"),
            (ThemeKind::Light, -1.0, "light-low"),
            (ThemeKind::Light, 1.0, "light-high"),
        ] {
            let context = h.ctx.clone();
            h.state_mut().session.prefs.appearance.brightness = brightness;
            h.state_mut().set_theme(&context, kind);
            h.run_steps(3);
            for node in h.root().children_recursive().filter(|node| matches!(node.accesskit_node().role(), Role::CheckBox | Role::Button)) {
                println!(
                    "{name}: {:?} {:?} disabled={} toggled={:?}",
                    node.accesskit_node().role(),
                    node.accesskit_node().label(),
                    node.accesskit_node().is_disabled(),
                    node.accesskit_node().toggled()
                );
            }
            h.render_serialized().unwrap().save(directory.join(format!("linked-import-{name}.png"))).unwrap();
        }
        let context = h.ctx.clone();
        h.state_mut().session.prefs.appearance.brightness = 0.0;
        h.state_mut().set_theme(&context, ThemeKind::Dark);
        h.run_steps(3);
    }

    select(&mut h, "Opening");
    select(&mut h, "Credits");
    link_selected(&mut h);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.project.items.len(), before + 2, "existing source links are not duplicated");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "the selected links are one edit");
    for name in ["Opening", "Credits"] {
        let item = h.state().session.project.items.values().find(|item| item.name == name).unwrap();
        assert_eq!(item.label, Label::Brown);
        assert!(matches!(&item.kind, ItemKind::Footage(footage) if footage.linked_comp.is_some()));
    }
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project.items.len(), before);
}

#[test]
fn cancel_restores_focus_and_a_changed_source_keeps_the_picker_open_with_an_error() {
    let (mut source, path) = source("linked-import-ui-cancel");
    for index in 0..40 {
        source.execute("comp.new", json!({"name":format!("Extra {index:02}"),"width":32,"height":24,"duration":2})).unwrap();
    }
    source.execute("file.save", json!({})).unwrap();
    let mut h = harness(&path);
    h.get_by_role_and_label(Role::TreeItem, "Main, Composition").focus();
    h.run_steps(3);
    assert!(h.get_by_role_and_label(Role::TreeItem, "Main, Composition").is_focused(), "the invoking row starts focused");
    let previous_focus = h.ctx.memory(|memory| memory.focused());
    let before = h.state().session.project.clone();
    open(&mut h);
    select(&mut h, "Opening");
    key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert!(std::sync::Arc::ptr_eq(&before, &h.state().session.project));
    assert_eq!(h.ctx.memory(|memory| memory.focused()), previous_focus, "Cancel restores the previous widget's real focus");
    assert!(h.get_by_role_and_label(Role::TreeItem, "Main, Composition").is_focused(), "Cancel returns focus to the invoking project row");

    open(&mut h);
    select(&mut h, "Opening");
    // Replace only this test's source after the list was shown: its name is no identity.
    source.execute("file.newProject", json!({})).unwrap();
    source.execute("comp.new", json!({"name":"Opening","width":32,"height":24,"duration":2})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    link_selected(&mut h);
    assert_eq!(h.state().dialog, Some(Dialog::LinkedImport));
    assert!(std::sync::Arc::ptr_eq(&before, &h.state().session.project), "a stale picker never imports another project");
    let alert = h.get_by_role(Role::Alert);
    assert!(h.ctx.content_rect().contains_rect(alert.rect()), "the error stays in view even with a long composition list");
    assert!(
        h.ctx.content_rect().contains_rect(h.get_by_role_and_label(Role::Button, "Link Selected").rect()),
        "the action buttons stay in view after a failed link"
    );
    h.get_by_role_and_label(Role::Button, "Reload List").click();
    h.run_steps(3);
    assert!(h.query_by_role(Role::Alert).is_none());
    assert!(h.get_by_role_and_label(Role::Button, "Link Selected").accesskit_node().is_disabled(), "refreshing the list clears stale choices");
}

#[test]
fn native_source_picker_can_answer_later_and_cancel_without_changing_the_project() {
    use std::sync::{Arc, Mutex};

    let (_source, path) = source("ui-linked-import-async");
    let mut h = harness(&path);
    h.state_mut().hooks.pick_files = Some(Box::new(|_| panic!("the native async picker must not use a blocking fallback")));
    let pending: Arc<Mutex<Vec<(FileDialog, DialogDone)>>> = Arc::default();
    let queue = pending.clone();
    h.state_mut().hooks.file_dialog_async = Some(Box::new(move |dialog, done| queue.lock().unwrap().push((dialog, done))));
    let before = h.state().session.project.clone();
    let context = h.ctx.clone();
    menus::invoke(h.state_mut(), &context, "file.linkComposition", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "the composition list waits for the native file dialog");
    let (dialog, done) = pending.lock().unwrap().pop().unwrap();
    assert_eq!(dialog, FileDialog::Open { exts: vec!["ecproj".into()], many: false });
    done(Ok(Vec::new()));
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(Arc::ptr_eq(&before, &h.state().session.project), "cancelling the native picker makes no edit");

    menus::invoke(h.state_mut(), &context, "file.linkComposition", json!({})).unwrap();
    let (_, done) = pending.lock().unwrap().pop().unwrap();
    let picked = path.to_string_lossy().to_string();
    std::thread::spawn(move || done(Ok(vec![picked]))).join().unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::LinkedImport));
    assert!(Arc::ptr_eq(&before, &h.state().session.project), "reading a saved source is not an import");
    select(&mut h, "Opening");
    h.get_by_role_and_label(Role::Button, "Choose Project…").click();
    h.run_steps(3);
    let (_, done) = pending.lock().unwrap().pop().unwrap();
    done(Ok(Vec::new()));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::LinkedImport), "cancelling a replacement keeps the existing list");
    assert_eq!(h.get_by_role_and_label(Role::CheckBox, "Opening").accesskit_node().toggled(), Some(Toggled::True));
    link_selected(&mut h);
    assert!(h.state().dialog.is_none());
    assert!(
        h.state().session.project.items.values().any(|item| item.name == "Opening" && matches!(&item.kind, ItemKind::Footage(f) if f.linked_comp.is_some()))
    );
}

#[test]
fn empty_and_already_linked_sources_remain_keyboard_usable_in_a_small_scaled_window() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/ui-linked-import-compact");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Source.ecproj");
    let mut source = Session::default();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
    let mut h = harness(&path);
    h.set_size(egui::vec2(960.0, 700.0));
    h.ctx.set_zoom_factor(2.0);
    h.run_steps(3);
    open(&mut h);
    assert!(h.query_by_label("This project contains no compositions.").is_some());
    assert!(h.get_by_role_and_label(Role::Button, "Link Selected").accesskit_node().is_disabled());
    assert!(h.get_by_role_and_label(Role::Button, "Choose Project…").is_focused(), "an empty list focuses the way to choose another project");

    let names = [format!("Opening {}", "long name ".repeat(18)), format!("Credits {}", "long name ".repeat(18))];
    for name in &names {
        source.execute("comp.new", json!({"name":name,"width":32,"height":24,"duration":2})).unwrap();
    }
    source.execute("file.save", json!({})).unwrap();
    h.get_by_role_and_label(Role::Button, "Reload List").click();
    h.run_steps(3);
    assert!(h.query_by_label("This project contains no compositions.").is_none());
    for name in &names {
        let row = h.get_by_role_and_label(Role::CheckBox, name).rect();
        assert!(row.is_positive() && h.ctx.content_rect().contains_rect(row), "long composition names remain within the compact list");
        select(&mut h, name);
    }
    for label in ["Cancel", "Link Selected"] {
        assert!(h.ctx.content_rect().contains_rect(h.get_by_role_and_label(Role::Button, label).rect()), "the scaled window keeps {label} in view");
    }
    link_selected(&mut h);
    open(&mut h);
    for name in &names {
        let label = format!("{name} — Already linked");
        let row = h.get_by_role_and_label(Role::CheckBox, &label);
        assert!(row.accesskit_node().is_disabled());
        assert_eq!(row.accesskit_node().toggled(), Some(Toggled::True));
    }
    assert!(h.get_by_role_and_label(Role::Button, "Link Selected").accesskit_node().is_disabled());
    assert!(h.get_by_role_and_label(Role::Button, "Choose Project…").is_focused(), "an entirely linked list still has an actionable keyboard target");
    key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
}
