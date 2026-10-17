//! The menu bar from the keyboard, and the app's shortcuts waiting while a menu is open (#279)
//! (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Press and release `key` (one frame each), then let the UI settle.
fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// The label of the widget with keyboard focus (the highlighted menu entry).
fn focused(h: &Harness<'_, EffectcraftApp>) -> String {
    h.query_by(|n| n.is_focused()).and_then(|n| n.accesskit_node().label()).unwrap_or_default().trim().to_string()
}

fn shown(h: &Harness<'_, EffectcraftApp>, label: &str) -> bool {
    h.query_by_label_contains(label).is_some()
}

/// Space (or any shortcut) while a menu is open goes to the menu, not the app: it started the
/// preview behind the open menu.
#[test]
fn shortcuts_wait_while_a_menu_is_open() {
    let mut h = harness();
    let edit = rect(&h, "menu.Edit");
    click_at(&mut h, edit.center());
    assert!(shown(&h, "Keyboard Shortcuts"), "the Edit menu is open");
    key(&mut h, Key::Space);
    assert!(!h.state().playback.playing, "Space didn't start the preview");
    // With the menu closed, Space previews again.
    key(&mut h, Key::Escape);
    assert!(!shown(&h, "Keyboard Shortcuts"), "Escape closed the menu");
    key(&mut h, Key::Space);
    assert!(h.state().playback.playing, "Space previews with the menus closed");
}

/// Alt focuses the menu bar; Down opens the menu, Up / Down move the highlight, Right opens a
/// submenu and Left closes it, Right on an entry opens the next menu, Escape closes them.
#[test]
fn arrow_keys_drive_the_menu_bar() {
    if cfg!(target_os = "macos") {
        // (macOS menus are the system's.)
        return;
    }
    let mut h = harness();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::ALT));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_eq!(focused(&h), "File", "Alt focused the menu bar");
    key(&mut h, Key::ArrowDown);
    assert!(shown(&h, "Open Project"), "Down opened File");
    assert!(focused(&h).starts_with("New"), "its first entry is highlighted: {:?}", focused(&h));
    key(&mut h, Key::ArrowRight);
    assert!(focused(&h).contains("New Project"), "Right opened the New submenu: {:?}", focused(&h));
    key(&mut h, Key::ArrowDown);
    assert!(focused(&h).contains("New Project from Template"), "Down moved down the submenu: {:?}", focused(&h));
    key(&mut h, Key::ArrowLeft);
    assert!(!shown(&h, "New Project from Template"), "Left closed the submenu");
    assert!(focused(&h).starts_with("New"), "and went back to its entry: {:?}", focused(&h));
    key(&mut h, Key::ArrowDown);
    assert!(focused(&h).contains("Open Project"), "{:?}", focused(&h));
    key(&mut h, Key::ArrowRight);
    assert!(!shown(&h, "Open Project"), "Right on an entry left File");
    assert!(shown(&h, "Keyboard Shortcuts"), "for Edit");
    key(&mut h, Key::ArrowLeft);
    assert!(shown(&h, "Open Project"), "Left went back to File");
    key(&mut h, Key::Escape);
    assert!(!shown(&h, "Open Project"), "Escape closed the menus");
    assert!(!h.state().playback.playing, "no key reached the app");
}

/// Up in a menu opened with the pointer highlights its last entry (wrapping around); Enter on a
/// submenu entry opens it with its first entry highlighted, and Enter chooses an entry.
#[test]
fn enter_chooses_the_highlighted_entry() {
    let mut h = harness();
    let edit = rect(&h, "menu.Edit");
    click_at(&mut h, edit.center());
    key(&mut h, Key::ArrowUp);
    if cfg!(target_os = "macos") {
        assert!(focused(&h).starts_with("Keyboard Shortcuts"), "Up went to the last entry: {:?}", focused(&h));
        key(&mut h, Key::Enter);
        assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Shortcuts), "Enter chose it");
    } else {
        assert!(focused(&h).starts_with("Preferences"), "Up went to the last entry: {:?}", focused(&h));
        key(&mut h, Key::Enter);
        assert!(focused(&h).starts_with("General"), "Enter opened the submenu: {:?}", focused(&h));
        key(&mut h, Key::Enter);
        assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Settings), "Enter chose General...");
    }
    assert!(!shown(&h, "Quick Apply"), "and closed the menu");
}

#[test]
fn relink_menu_uses_engine_availability_and_folder_picker_cancel_is_a_noop() {
    use std::cell::Cell;
    use std::rc::Rc;

    let mut app = EffectcraftApp::new(Session::default());
    let menu = effectcraft_ui_egui::menus::menu_items(&app);
    let entry = menu.iter().find(|e| e.id == "file.relinkFootage").unwrap();
    assert_eq!(entry.path, ["File", "Dependencies"]);
    assert_eq!(entry.enabled, app.session.is_enabled("file.relinkFootage"));
    assert_eq!(entry.enabled, !cfg!(target_arch = "wasm32"));
    let picked = Rc::new(Cell::new(false));
    let flag = picked.clone();
    app.hooks.pick_folder = Some(Box::new(move || {
        flag.set(true);
        None
    }));
    let revision = app.session.revision;
    let result = effectcraft_ui_egui::menus::invoke(&mut app, &egui::Context::default(), "file.relinkFootage", json!({})).unwrap();
    assert!(picked.get());
    assert!(result.is_null());
    assert_eq!(app.session.revision, revision);
    assert!(app.session.history.undo.is_empty());
}

#[test]
fn relink_is_reachable_through_the_dependencies_menu() {
    use std::cell::Cell;
    use std::rc::Rc;

    let mut h = harness();
    let picked = Rc::new(Cell::new(false));
    let flag = picked.clone();
    h.state_mut().hooks.pick_folder = Some(Box::new(move || {
        flag.set(true);
        None
    }));
    let revision = h.state().session.revision;
    let file = rect(&h, "menu.File");
    click_at(&mut h, file.center());
    let dependencies = h.query_by_label_contains("Dependencies").unwrap().rect();
    h.input_mut().events.push(Event::PointerMoved(dependencies.center()));
    h.run_steps(8);
    let relink = h.query_by_label_contains("Relink Missing Footage").unwrap().rect();
    if let Some(path) = std::env::var_os("EFFECTCRAFT_RELINK_MENU_SNAPSHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    click_at(&mut h, relink.center());
    assert!(picked.get());
    assert_eq!(h.state().session.revision, revision);
}

#[test]
fn relink_restores_decoded_pixels_and_invalidates_viewer_content() {
    use effectcraft_engine::project::{ItemId, ItemKind};
    use effectcraft_engine::render::RenderOpts;
    use effectcraft_engine::time::Tick;

    let dir = std::env::temp_dir().join(format!("ec-ui-relink-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("old")).unwrap();
    std::fs::create_dir_all(dir.join("new/deep")).unwrap();
    let old = dir.join("old/plate.png");
    image::RgbaImage::from_pixel(32, 32, image::Rgba([230, 40, 80, 255])).save(&old).unwrap();
    let mut s = effectcraft_host::session();
    let item = s.execute("file.import", json!({"paths": [old], "sequence": false})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Relink", "width": 32, "height": 32, "duration": 1})).unwrap();
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let expected = s.render(cid, Tick::ZERO, RenderOpts::default());
    let moved = dir.join("new/deep/plate.png");
    std::fs::rename(&old, &moved).unwrap();
    s.execute("footage.check", json!({"wait": true})).unwrap();
    let ItemKind::Footage(f) = &s.project.item(ItemId(item)).unwrap().kind else { panic!("footage") };
    assert!(f.missing);
    let missing_content = effectcraft_ui_egui::frames::comp_content(&s.project, cid);
    let missing = s.render(cid, Tick::ZERO, RenderOpts::default());
    assert_ne!(missing.data, expected.data);
    let r = s.execute("file.relinkFootage", json!({"folder": dir.join("new"), "wait": true})).unwrap();
    assert_eq!(r["relinked"], 1);
    assert_ne!(effectcraft_ui_egui::frames::comp_content(&s.project, cid), missing_content);
    assert_eq!(s.render(cid, Tick::ZERO, RenderOpts::default()).data, expected.data);
    std::fs::remove_dir_all(dir).unwrap();
}

/// #767, #1701: File ▸ Import ▸ File… and the proxy dialog offer every movie type the importer
/// reads (MXF from broadcast cameras and Avid, AVI, …), the list the Media Browser goes by too.
#[test]
fn import_and_proxy_dialogs_offer_every_movie_type() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut app = EffectcraftApp::new(Session::default());
    let offered = Rc::new(RefCell::new(Vec::<String>::new()));
    let seen = offered.clone();
    app.hooks.pick_files = Some(Box::new(move |exts: &[&str]| {
        *seen.borrow_mut() = exts.iter().map(|e| e.to_string()).collect();
        Vec::new()
    }));
    for (command, still) in [("file.import", "psd"), ("file.setProxy", "png")] {
        let result = effectcraft_ui_egui::menus::invoke(&mut app, &egui::Context::default(), command, json!({})).unwrap();
        assert!(result.is_null(), "{command} cancelled: nothing done");
        let offered = offered.borrow();
        for ext in effectcraft_project::VIDEO_EXTENSIONS.iter().chain([&still]) {
            assert!(offered.iter().any(|e| e == ext), "{command} offers {ext}: {offered:?}");
        }
        assert!(offered.iter().any(|e| e == "avi"), "{command}: {offered:?}");
    }
}

/// #1059: opening the demo from the start screen and immediately pressing Spacebar previews
/// the demo — the start screen's keyboard focus must not outlive it.
#[test]
fn space_previews_after_opening_the_demo_from_home() {
    let mut h = {
        let s = Session::default();
        let mut app = EffectcraftApp::new(s);
        app.ui.start_screen = true;
        Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_| app)
    };
    h.run_steps(3);
    let demo = rect(&h, "home.file.openDemoProject");
    click_at(&mut h, demo.center());
    assert!(h.state().session.active_comp().is_some(), "the demo comp is open");
    assert!(!h.state().ui.start_screen, "the start screen closed");
    key(&mut h, Key::Space);
    assert!(h.state().playback.playing, "Space previews the demo (#1059)");
}
