//! The menu bar from the keyboard, and the app's shortcuts waiting while a menu is open (#279)
//! (egui_kittest, UI logic only).

use crate::gpu_test::RenderSerial;
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

pub(crate) fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

pub(crate) fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Press the primary button at `p` and keep it down.
pub(crate) fn press(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(2);
}

/// Move the pointer to `p` (press or no press, whichever is held).
pub(crate) fn move_to(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.run_steps(2);
}

/// Release the primary button at `p`.
fn release(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
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

#[test]
fn browse_presets_clears_search_and_scrolls_to_the_visible_presets_list() {
    use effectcraft_ui_egui::dock::PanelKind;

    let mut h = harness();
    let ctx = h.ctx.clone();
    let favorites: Vec<String> = effectcraft_engine::effects::all().iter().map(|effect| effect.id.to_string()).collect();
    assert!(!favorites.is_empty(), "the test needs registered effects");
    let recent: Vec<String> = favorites.iter().take(8).cloned().collect();

    h.state_mut().session.prefs.effects_favorites = favorites;
    h.state_mut().ui.effects_recent = recent;
    h.state_mut().ui.effects_open.extend(["favorites".into(), "recent".into()]);
    h.state_mut().ui.effects_search = "no matching animation preset".into();
    h.state_mut().ui.effects_view.presets = true;

    let managed = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "effect.manage", json!({}));
    assert!(managed.is_ok(), "{managed:?}");
    assert_eq!(h.state().ui.focused, PanelKind::EffectsPresets);
    assert_eq!(h.state().ui.effects_search, "no matching animation preset", "Manage Effects keeps its existing search behavior");
    assert!(h.state().ui.effects_view.presets, "Manage Effects keeps the existing presets setting");

    h.run_steps(3);
    assert!(h.state().auto.find("effects.category.presets").is_none(), "the unmatched search hides Animation Presets");
    h.state_mut().ui.effects_view.presets = false;

    let result = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "anim.browsePresets", json!({}));

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(h.state().ui.focused, PanelKind::EffectsPresets);
    assert!(h.state().ui.effects_search.is_empty());
    assert!(h.state().ui.effects_view.presets);
    assert!(h.state().ui.effects_open.contains("presets"));

    h.run_steps(3);
    assert!(
        h.state().auto.find("effects.category.presets").is_some(),
        "Animation Presets must be visible after Browse Presets, even below expanded Favorites and Recently Used"
    );
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
        h.render_serialized().unwrap().save(path).unwrap();
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

/// The titles open on the press, not the release, so the menus can be browsed with the button
/// still held down: moving onto an entry and releasing there chooses it (as in After Effects).
#[test]
fn a_menu_opens_on_press_and_browsing_releases_choose_an_entry() {
    let mut h = harness();
    let edit = rect(&h, "menu.Edit");
    press(&mut h, edit.center());
    assert!(shown(&h, "Keyboard Shortcuts"), "the Edit menu opens on the press, before any release");
    let item = h.query_by_label_contains("Keyboard Shortcuts").expect("the Edit menu is open").rect();
    move_to(&mut h, item.center());
    release(&mut h, item.center());
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Shortcuts), "the release over the entry chose it");
    assert!(!shown(&h, "Quick Apply"), "and closed the menu");
}

/// While a menu is open, hovering another title switches to that menu (native menu bars).
#[test]
fn hovering_another_title_while_a_menu_is_open_switches_to_it() {
    let mut h = harness();
    let edit = rect(&h, "menu.Edit");
    click_at(&mut h, edit.center());
    assert!(shown(&h, "Keyboard Shortcuts"), "the Edit menu is open");
    let file = rect(&h, "menu.File");
    move_to(&mut h, file.center());
    assert!(shown(&h, "Open Project"), "hovering File opened the File menu");
    assert!(!shown(&h, "Keyboard Shortcuts"), "and closed Edit");
}

/// The press on the title of the open menu closes it again, and the click's release — a plain
/// click on the title — leaves it closed.
#[test]
fn pressing_the_open_title_again_closes_its_menu() {
    let mut h = harness();
    let file = rect(&h, "menu.File");
    click_at(&mut h, file.center());
    assert!(shown(&h, "Open Project"), "the File menu is open");
    click_at(&mut h, file.center());
    assert!(!shown(&h, "Open Project"), "the second press closed it");
}

/// Press-and-hold browse (AE's gesture) reaches into a submenu: while the button is still held,
/// moving onto the submenu row opens it, and the release inside the submenu picks the entry.
#[test]
fn hold_browse_opens_a_submenu_and_the_release_inside_picks_an_entry() {
    let mut h = harness();
    // Applying an effect needs a selected layer, as in the app after clicking one in the timeline.
    let first = {
        let cid = h.state().session.active_comp_id().expect("the demo opens a comp");
        let comp = h.state().session.project.comp(cid).expect("the demo comp exists");
        comp.layers.iter().find(|l| l.effects().is_some()).expect("the demo comp has a layer that takes effects").id
    };
    h.state_mut().session.state.selected_layers = vec![first];
    let effect = rect(&h, "menu.Effect");
    press(&mut h, effect.center());
    // (Effect ▸ Remove All: "Effect Controls" also names the panel's tab.)
    assert!(shown(&h, "Remove All"), "the Effect menu opens on the press");
    // The categories come from the effect registry; the first alphabetically is "3D Channel".
    let cat = h.query_by_label_contains("3D Channel").expect("the category row is shown").rect();
    move_to(&mut h, cat.center());
    assert!(shown(&h, "Cryptomatte"), "moving onto a category while still holding opened its submenu");
    let entry = h.query_by_label_contains("Cryptomatte").expect("the submenu is open").rect();
    move_to(&mut h, entry.center());
    release(&mut h, entry.center());
    assert!(!shown(&h, "Remove All"), "the release closed the menus");
    assert!(h.state().session.state.last_effect.is_some(), "and the release chose the entry: it applied its effect");
}

/// The open menus close on a press outside them and when the window loses focus. The pointer
/// leaving the window — onto the OS title bar, whose clicks are never forwarded to egui — closes
/// nothing, however long it stays away: there is no click to close on, and a graze or a stay
/// must not drop the menu. The closing press itself is not swallowed: it reaches the widget under
/// it, as a click does in After Effects.
#[test]
fn outside_presses_and_focus_loss_close_the_menus() {
    let mut h = harness();
    let file = rect(&h, "menu.File");
    click_at(&mut h, file.center());
    assert!(shown(&h, "Open Project"));
    // A press in the empty part of the bar, outside every title and menu.
    click_at(&mut h, pos2(1100.0, 12.0));
    assert!(!shown(&h, "Open Project"), "a press outside the menus closes them");
    // The press reaches the header's workspace tab behind the open menu: the menu closes and the
    // tab still acts (the menus don't swallow input).
    click_at(&mut h, file.center());
    assert!(shown(&h, "Open Project"));
    let tab = rect(&h, "header.workspace.Review");
    click_at(&mut h, tab.center());
    assert_eq!(h.state().ui.workspace, "Review", "the press reached the tab behind the open menu");
    assert!(!shown(&h, "Open Project"), "and closed the menu");
    // Moving onto the OS title bar fires the cursor-left event — its clicks are never forwarded
    // to egui — which egui reports as `PointerGone`. It must not close the menus, however long
    // the pointer stays away.
    click_at(&mut h, file.center());
    assert!(shown(&h, "Open Project"));
    h.input_mut().events.push(Event::PointerGone);
    h.run_steps(1);
    assert!(shown(&h, "Open Project"), "leaving the window alone doesn't drop the menus at once");
    let now = h.ctx.input(|i| i.time);
    h.input_mut().time = Some(now + 60.0);
    h.run_steps(2);
    assert!(shown(&h, "Open Project"), "staying away doesn't close them either");
    h.input_mut().events.push(Event::PointerMoved(file.center()));
    h.run_steps(2);
    assert!(shown(&h, "Open Project"), "the pointer coming back keeps them open too");
    // Losing focus (alt-tabbing away, or clicking another window).
    h.input_mut().events.push(Event::WindowFocused(false));
    h.run_steps(2);
    assert!(!shown(&h, "Open Project"), "losing focus closes the menus");
}

/// From an open submenu, the pointer reaching another title switches menus too (#1290).
#[test]
fn hovering_a_title_from_an_open_submenu_switches_menus() {
    let mut h = harness();
    let (file, edit) = (rect(&h, "menu.File"), rect(&h, "menu.Edit"));
    click_at(&mut h, file.center());
    let new = h.query_by(|n| n.label().is_some_and(|l| l.trim_end_matches('\u{23f5}').trim() == "New")).expect("File ▸ New").rect();
    move_to(&mut h, new.center());
    h.run_steps(5);
    assert!(shown(&h, "New Project from Template"), "the New submenu is open");
    move_to(&mut h, edit.center());
    assert!(shown(&h, "Keyboard Shortcuts"), "hovering Edit opened it");
    assert!(!shown(&h, "New Project from Template") && !shown(&h, "Open Project"), "and closed File and its submenu");
}

/// A title the pointer can't reach (the app is disabled under a dialog) doesn't switch menus
/// (#1290).
#[test]
fn a_disabled_title_does_not_switch_menus() {
    let mut h = harness();
    let (file, edit) = (rect(&h, "menu.File"), rect(&h, "menu.Edit"));
    click_at(&mut h, file.center());
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    h.run_steps(2);
    move_to(&mut h, edit.center());
    assert!(!shown(&h, "Keyboard Shortcuts"), "Edit didn't open");
}

/// A pointer resting on a title doesn't undo Left / Right: only a moving pointer switches
/// (#1290).
#[test]
fn a_resting_pointer_does_not_override_the_arrow_keys() {
    if cfg!(target_os = "macos") {
        // (Right / Left between menus are tested where the in-window bar is the app's menu.)
        return;
    }
    let mut h = harness();
    let file = rect(&h, "menu.File");
    click_at(&mut h, file.center());
    key(&mut h, Key::ArrowRight);
    h.run_steps(3);
    assert!(shown(&h, "Keyboard Shortcuts"), "Right opened Edit with the pointer still on File");
    assert!(!shown(&h, "Open Project"));
}

/// Leaving the window keeps the menu open, and coming back in over another title switches to it
/// (#1290): egui reports no pointer movement for that first position.
#[test]
fn pointer_reentering_over_another_title_switches_menus() {
    let mut h = harness();
    let (file, edit) = (rect(&h, "menu.File"), rect(&h, "menu.Edit"));
    click_at(&mut h, file.center());
    h.input_mut().events.push(Event::PointerGone);
    h.run_steps(2);
    assert!(shown(&h, "Open Project"), "leaving the window kept File open");
    move_to(&mut h, edit.center());
    assert!(shown(&h, "Keyboard Shortcuts"), "coming back over Edit opened it");
    assert!(!shown(&h, "Open Project"), "and closed File");
}
