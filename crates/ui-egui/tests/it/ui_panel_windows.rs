//! Multiple monitors (#486): Move Panel to New Window puts a panel in a floating window of its
//! own, which on desktop is an OS window that can go on another screen, and Move Panel to Main
//! Window docks it again. The test harness embeds viewports, so these windows draw in the main
//! window here, as on the web.
use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{PanelKind, WorkspaceExt as _};
use effectcraft_ui_egui::dock_ui::sanitized_window_rect;
use serde_json::json;

fn invoke(app: &mut EffectcraftApp, ctx: &egui::Context, params: serde_json::Value) -> Result<serde_json::Value, String> {
    effectcraft_ui_egui::menus::invoke(app, ctx, "window.panelWindow", params)
}

fn native(app: &EffectcraftApp, panel: PanelKind) -> Option<[f32; 4]> {
    app.ui.dock.layout.floating.iter().find(|w| w.contains(&panel)).and_then(|w| w.native)
}

#[test]
fn panel_window_command_moves_a_docked_panel_into_its_own_window_and_back() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    assert!(app.ui.dock.is_docked(PanelKind::Project));
    assert_eq!(invoke(&mut app, &ctx, json!({"panel": "Project", "window": true})).unwrap(), json!({"window": true}));
    // Undocked first, then given a window of its own.
    assert!(!app.ui.dock.is_docked(PanelKind::Project));
    assert!(app.ui.dock.layout.floating.iter().any(|w| w.panels().copied().collect::<Vec<_>>() == [PanelKind::Project]));
    assert!(app.in_panel_window(PanelKind::Project));
    let rect = native(&app, PanelKind::Project).unwrap();
    assert_eq!(rect, sanitized_window_rect(rect));
    // Asking again changes nothing; no `window` toggles.
    assert_eq!(invoke(&mut app, &ctx, json!({"panel": "Project", "window": true})).unwrap(), json!({"window": true}));
    assert_eq!(invoke(&mut app, &ctx, json!({"panel": "Project"})).unwrap(), json!({"window": false}));
    // Back in the main window: docked in the first docked group.
    assert!(!app.in_panel_window(PanelKind::Project));
    assert!(!app.ui.dock.is_floating(PanelKind::Project));
    assert!(app.ui.dock.is_docked(PanelKind::Project));
    // The serialized panel form works too.
    assert_eq!(invoke(&mut app, &ctx, json!({"panel": PanelKind::Project})).unwrap(), json!({"window": true}));
    // Bad requests are errors and change nothing.
    let before = app.ui.dock.clone();
    for params in [json!({}), json!({"panel": "Nope"}), json!({"panel": 7}), json!({"panel": "Project", "window": "yes"}), json!({"panel": {"Viewer": 9}})] {
        assert!(invoke(&mut app, &ctx, params.clone()).is_err(), "{params} must be rejected");
    }
    assert_eq!(app.ui.dock, before);
    // Docking it back into the main window ends its window.
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "window.dockPanel", json!({"panel": "Project", "anchor": "Timeline"})).unwrap();
    assert!(!app.in_panel_window(PanelKind::Project));
    assert!(app.ui.dock.is_docked(PanelKind::Project));
}

#[test]
fn panel_windows_survive_restart_with_hostile_places_made_safe() {
    use effectcraft_engine::config::MemoryConfig;
    use std::sync::Arc;
    let store = Arc::new(MemoryConfig::default());
    let mut app = EffectcraftApp::new(Session { config: Some(store.clone()), ..Default::default() });
    let ctx = egui::Context::default();
    invoke(&mut app, &ctx, json!({"panel": "Project", "window": true})).unwrap();
    let hostile = [90_000.0, -99_000.0, 99_000.0, 0.5];
    app.apply_dock(craft_ui::docking::Action::SetNative { panel: PanelKind::Project, rect: Some(hostile) }).unwrap();
    eframe::App::on_exit(&mut app);
    let restarted = EffectcraftApp::new(Session { config: Some(store), ..Default::default() });
    assert_eq!(native(&restarted, PanelKind::Project), Some(sanitized_window_rect(hostile)));
    let [x, y, w, h] = sanitized_window_rect(hostile);
    assert!(x <= 100_000.0 && y >= -100_000.0 && (160.0..=16_384.0).contains(&w) && (100.0..=16_384.0).contains(&h));
    assert!(sanitized_window_rect([f32::NAN, f32::INFINITY, f32::NAN, f32::NEG_INFINITY]).iter().all(|v| v.is_finite()));
}

#[test]
fn a_click_on_a_floating_panel_does_nothing_while_a_dialog_is_open() {
    use egui::{Event, Modifiers, PointerButton, pos2, vec2};
    use egui_kittest::Harness;
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.start_screen = false;
    app.apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::Project, rect: [80.0, 120.0, 320.0, 240.0] }).unwrap();
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |_| app);
    h.run_steps(3);
    let close = |h: &Harness<'_, EffectcraftApp>| {
        let e = h.state().auto.elements.iter().find(|e| e.id.starts_with("panel.float.") && e.id.ends_with(".close")).expect("the floating window's ×");
        pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
    };
    let click = |h: &mut Harness<'_, EffectcraftApp>, pos| {
        h.event(Event::PointerMoved(pos));
        h.step();
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    let at = close(&h);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "general"})).unwrap();
    h.run_steps(3);
    let before = h.state().ui.dock.clone();
    click(&mut h, at);
    assert_eq!(h.state().ui.dock, before, "the floating panel takes no click while Settings is open");
    assert!(h.state().ui.dock.is_floating(PanelKind::Project));
    // The same click closes the window once the dialog is gone.
    h.state_mut().dialog = None;
    h.run_steps(3);
    let at = close(&h);
    click(&mut h, at);
    assert!(!h.state().ui.dock.is_open(PanelKind::Project));
}
