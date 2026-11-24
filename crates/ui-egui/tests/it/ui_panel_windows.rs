//! Multiple monitors (#486): Move Panel to New Window puts a panel in a floating window of its
//! own, which on desktop is an OS window that can go on another screen, and Move Panel to Main
//! Window docks it again. The test harness embeds viewports, so these windows draw in the main
//! window here, as on the web.
use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{PanelKind, WorkspaceExt as _};
use serde_json::json;

fn invoke(app: &mut EffectcraftApp, ctx: &egui::Context, params: serde_json::Value) -> Result<serde_json::Value, String> {
    effectcraft_ui_egui::menus::invoke(app, ctx, "window.panelWindow", params)
}

#[test]
fn panel_window_command_moves_a_docked_panel_into_its_own_window_and_back() {
    let mut app = EffectcraftApp::new(Session::default());
    let ctx = egui::Context::default();
    assert!(app.ui.dock.is_docked(PanelKind::Project));
    assert_eq!(invoke(&mut app, &ctx, json!({"panel": "Project", "window": true})).unwrap(), json!({"window": true}));
    // Undocked first, then given a window of its own.
    assert!(!app.ui.dock.is_docked(PanelKind::Project));
    assert!(app.ui.dock.layout().floating.iter().any(|w| w.panels().copied().collect::<Vec<_>>() == [PanelKind::Project]));
    assert!(app.in_panel_window(PanelKind::Project));
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
fn a_drag_on_a_floating_panel_does_nothing_while_a_dialog_is_open() {
    use egui::{Event, Modifiers, PointerButton, vec2};
    use egui_kittest::Harness;
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.start_screen = false;
    app.apply_dock(craft_behavior::dock::Action::float(PanelKind::Project, effectcraft_ui_egui::dock_command::rect_of([80.0, 120.0, 320.0, 240.0]))).unwrap();
    let mut h =
        Harness::builder().with_size(vec2(1400.0, 900.0)).with_context_setup(effectcraft_ui_egui::framework::install_headless).build_eframe(move |_| app);
    h.run_steps(3);
    // The floating window's move grip: a drag moves it only while no dialog is up.
    let grip = |h: &Harness<'_, EffectcraftApp>| {
        let e = h.state().auto.find("panel.float.0.move").expect("the floating window's move grip");
        egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
    };
    let drag = |h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2| {
        let to = from + vec2(-30.0, 20.0);
        h.event(Event::PointerMoved(from));
        h.step();
        h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=5 {
            h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 5.0)));
            h.step();
        }
        h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
        h.run_steps(2);
    };
    let at = grip(&h);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "general"})).unwrap();
    h.run_steps(3);
    let before = h.state().ui.dock.clone();
    drag(&mut h, at);
    assert_eq!(h.state().ui.dock, before, "the floating panel takes no drag while Settings is open");
    // The same drag moves the window once the dialog is gone.
    h.state_mut().dialog = None;
    h.run_steps(3);
    let at = grip(&h);
    drag(&mut h, at);
    assert_ne!(h.state().ui.dock, before, "the drag moved the floating window");
    assert!(h.state().ui.dock.is_floating(PanelKind::Project));
}
