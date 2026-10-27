//! View ▸ New Viewer: several Composition viewers. New Viewer locks the viewer in use; opening a
//! comp goes to an unlocked viewer (a new one when every viewer is locked); a click on another
//! viewer makes it the active one; closing a viewer forgets it.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::dock::WorkspaceExt as _;
use effectcraft_ui_egui::panels::viewers;
use egui::{Event, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, p: Value) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, p).unwrap();
    h.run_steps(3);
}

/// Hover element `id`; returns its centre.
fn hover(h: &mut Harness<'_, EffectcraftApp>, id: &str) -> egui::Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let p = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    p
}

fn button(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
}

/// Click element `id`, the release a frame after the press (as a hand does: a press in a tab
/// strip focuses the shown panel first, the click then the tab).
fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = hover(h, id);
    button(h, p, true);
    h.step();
    button(h, p, false);
    h.run_steps(3);
}

/// Double-click element `id` (in one frame: the harness' frame time is longer than a double-click's).
fn double_click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = hover(h, id);
    for pressed in [true, false, true, false] {
        button(h, p, pressed);
    }
    h.run_steps(3);
}

fn shows(h: &Harness<'_, EffectcraftApp>, viewer: u32) -> Option<u64> {
    viewers::comp_of(h.state(), viewer).map(|c| c.0)
}

fn new_comp(s: &mut Session, name: &str) -> u64 {
    s.execute("comp.new", json!({"name": name, "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap()
}

#[test]
fn new_viewers_lock_route_comps_and_activate_on_click() {
    let mut s = Session::default();
    let c = new_comp(&mut s, "C");
    let b = new_comp(&mut s, "B");
    let a = new_comp(&mut s, "A");
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    // View ▸ New Viewer: viewer 1 shows A too, viewer 0 (the Composition panel) is locked.
    invoke(&mut h, "view.newViewer", json!({}));
    assert!(h.state().ui.dock.is_docked(PanelKind::Viewer(1)));
    assert!(viewers::locked(h.state(), 0) && !viewers::locked(h.state(), 1));
    assert_eq!((h.state().ui.active_viewer, shows(&h, 0), shows(&h, 1)), (1, Some(a), Some(a)));
    // Side by side, so both draw.
    h.state_mut()
        .apply_dock(craft_ui::docking::Action::Move {
            panel: PanelKind::Viewer(1),
            anchor: PanelKind::Composition,
            placement: craft_ui::docking::Placement::Split(craft_ui::docking::Zone::Right),
        })
        .unwrap();
    h.state_mut().apply_dock(craft_ui::docking::Action::Activate { panel: PanelKind::Composition }).unwrap();
    h.run_steps(3);
    // Opening B shows it in the active (unlocked) viewer; the locked one keeps A.
    invoke(&mut h, "comp.open", json!({"comp": b}));
    assert_eq!((h.state().ui.active_viewer, shows(&h, 0), shows(&h, 1)), (1, Some(a), Some(b)));
    assert!(h.state().auto.find("viewers.0").is_some(), "the other viewer draws passively");
    // A click on viewer 0 makes it active, and A the active comp.
    click(&mut h, "viewers.0");
    assert_eq!(h.state().ui.active_viewer, 0);
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(a)));
    assert_eq!(shows(&h, 1), Some(b));
    // Viewer 0 is locked to A: opening C goes to the unlocked viewer 1.
    invoke(&mut h, "comp.open", json!({"comp": c}));
    assert_eq!((h.state().ui.active_viewer, shows(&h, 0), shows(&h, 1)), (1, Some(a), Some(c)));
    // Every viewer locked: opening B makes a new viewer.
    h.state_mut().ui.locked_tabs.insert(PanelKind::Viewer(1).id());
    invoke(&mut h, "comp.open", json!({"comp": b}));
    assert_eq!(h.state().ui.active_viewer, 2);
    assert!(h.state().ui.dock.is_docked(PanelKind::Viewer(2)));
    assert_eq!((shows(&h, 0), shows(&h, 1), shows(&h, 2)), (Some(a), Some(c), Some(b)));
    // Closing the active viewer forgets it; another open viewer takes over.
    h.state_mut().close_panel(PanelKind::Viewer(2));
    h.run_steps(2);
    assert!(!h.state().ui.viewers.contains_key(&2));
    // Viewer 0 takes over with its own comp (it is locked to A), which becomes the active comp.
    assert_eq!(h.state().ui.active_viewer, 0);
    assert_eq!((shows(&h, 0), h.state().session.active_comp_id()), (Some(a), Some(ItemId(a))));
    // The tab names the comp each viewer shows.
    let titles: Vec<String> = h
        .state()
        .auto
        .elements
        .iter()
        .filter(|e| e.id.starts_with("panel.tab.Composition") || e.id.starts_with("panel.tab.Viewer"))
        .map(|e| e.label.clone())
        .collect();
    assert!(titles.iter().any(|t| t == "Composition A") && titles.iter().any(|t| t == "Composition C"), "{titles:?}");
}

/// Issue #156: a locked viewer keeps its comp when the Project panel opens another one. The
/// Project panel opens it mid-frame, before the viewers draw; the active (locked) viewer used to
/// record the new comp as its own then, so the lock no longer applied.
#[test]
fn a_locked_viewer_keeps_its_comp_when_the_project_panel_opens_another() {
    let mut s = Session::default();
    let b = new_comp(&mut s, "B");
    let a = new_comp(&mut s, "A");
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    // Viewer 0 locked to A, and B in viewer 1 (a tab next to it).
    invoke(&mut h, "view.newViewer", json!({}));
    invoke(&mut h, "comp.open", json!({"comp": b}));
    // Back to the locked viewer by its tab.
    click(&mut h, "panel.tab.Composition");
    assert!(viewers::locked(h.state(), 0));
    assert_eq!((h.state().ui.active_viewer, shows(&h, 0), shows(&h, 1)), (0, Some(a), Some(b)));
    // Double-click B in the Project panel: it shows in its viewer, the locked one keeps A.
    double_click(&mut h, &format!("project.item.{b}.name"));
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(b)));
    assert_eq!((h.state().ui.active_viewer, shows(&h, 0), shows(&h, 1)), (1, Some(a), Some(b)));
}

/// `cargo test -p effectcraft-ui-egui --test it ui_viewers:: -- --ignored`: two viewers side by side
/// (`target/test-out/viewers.png`).
#[test]
#[ignore]
fn viewers_snapshot() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let other = s.project.items.values().filter(|i| i.as_comp().is_some()).map(|i| i.id).find(|id| Some(*id) != s.active_comp_id()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    invoke(&mut h, "view.newViewer", json!({}));
    h.state_mut()
        .apply_dock(craft_ui::docking::Action::Move {
            panel: PanelKind::Viewer(1),
            anchor: PanelKind::Composition,
            placement: craft_ui::docking::Placement::Split(craft_ui::docking::Zone::Right),
        })
        .unwrap();
    h.state_mut().apply_dock(craft_ui::docking::Action::Activate { panel: PanelKind::Composition }).unwrap();
    invoke(&mut h, "comp.open", json!({"comp": other.0}));
    for _ in 0..300 {
        h.step();
        if h.state().frames.inflight() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.run_steps(4);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out");
    std::fs::create_dir_all(dir).unwrap();
    h.render().unwrap().save(format!("{dir}/viewers.png")).unwrap();
}
