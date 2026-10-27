//! EffectCraft's workspaces in craft-ui's shared docking: transactions, the command path, pointer
//! gestures and persistence.
use craft_ui::docking::{Action, Node, Placement, StackEntry, Zone};
use effectcraft_engine::Session;
use effectcraft_ui_egui::{
    EffectcraftApp,
    dock::{self, Layout, PanelKind, SplitSize, Workspace, WorkspaceExt as _},
};
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn invoke(app: &mut EffectcraftApp, id: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    effectcraft_ui_egui::menus::invoke(app, &egui::Context::default(), id, params)
}

fn floating(app: &EffectcraftApp) -> &[dock::Floating<PanelKind>] {
    &app.ui.dock.layout.floating
}

#[test]
fn shared_move_preserves_accordion_metadata_and_unrelated_active_tabs() {
    let mut workspace = dock::docked(dock::hsplit(
        SplitSize::FixedSecond(300.0),
        dock::tabs(&[PanelKind::Project, PanelKind::Composition, PanelKind::Layer], 1),
        dock::stack(&[(PanelKind::Preview, false, Some(73.0)), (PanelKind::Properties, true, Some(211.0)), (PanelKind::Align, false, None)]),
    ));
    workspace.apply(Action::Close { panel: PanelKind::Project }, dock::permissions).unwrap();
    assert!(workspace.is_visible(PanelKind::Composition));
    workspace
        .apply(Action::Move { panel: PanelKind::Preview, anchor: PanelKind::Layer, placement: Placement::Tab { before: None } }, dock::permissions)
        .unwrap();
    let Some(Node::Split { second, size, .. }) = &workspace.layout.root else { panic!("split was lost") };
    assert_eq!(*size, SplitSize::FixedSecond(300.0));
    let Node::Stack { entries } = second.as_ref() else { panic!("stack was lost") };
    // The unrelated stacked panels keep their exact state.
    assert_eq!(
        entries,
        &vec![StackEntry { panel: PanelKind::Properties, open: true, height: Some(211.0) }, StackEntry { panel: PanelKind::Align, open: false, height: None }]
    );
    assert_eq!(workspace.group_of(PanelKind::Preview), vec![PanelKind::Composition, PanelKind::Layer, PanelKind::Preview]);
    // Rejected moves and geometry change nothing.
    let before = workspace.clone();
    assert!(
        workspace
            .apply(Action::Move { panel: PanelKind::Preview, anchor: PanelKind::Audio, placement: Placement::Split(Zone::Left) }, dock::permissions)
            .is_err()
    );
    assert!(workspace.apply(Action::Float { panel: PanelKind::Preview, rect: [0.0, 0.0, f32::INFINITY, 200.0] }, dock::permissions).is_err());
    assert_eq!(workspace, before);
}

#[test]
fn every_builtin_workspace_validates_and_roundtrips_with_dynamic_identities() {
    for name in dock::WORKSPACES {
        let workspace = Workspace::new(dock::workspace(name));
        workspace.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(workspace.is_open(PanelKind::Composition), "{name} lacks the Composition panel");
        let text = serde_json::to_string(&workspace).unwrap();
        assert_eq!(serde_json::from_str::<Workspace>(&text).unwrap(), workspace, "{name}");
    }
    assert_eq!(dock::workspace("Undocked Panels").floating.len(), 4);
    // Built again, a built-in layout has the same windows (ids included).
    let ids = |layout: &Layout| layout.floating.iter().map(|w| w.id).collect::<Vec<_>>();
    assert_eq!(ids(&dock::workspace("Undocked Panels")), ids(&dock::workspace("Undocked Panels")));
    let mut workspace = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::Viewer(42), PanelKind::ScriptPanel(77)], 1));
    workspace.apply(Action::Float { panel: PanelKind::ScriptPanel(77), rect: [20.0, 30.0, 400.0, 300.0] }, dock::permissions).unwrap();
    let text = serde_json::to_string(&workspace).unwrap();
    assert!(text.contains("\"ScriptPanel\":77") && text.contains("\"Viewer\":42"), "{text}");
    assert_eq!(serde_json::from_str::<Workspace>(&text).unwrap(), workspace);
    workspace
        .apply(Action::Move { panel: PanelKind::ScriptPanel(77), anchor: PanelKind::Viewer(42), placement: Placement::Tab { before: None } }, dock::permissions)
        .unwrap();
    assert!(workspace.layout.floating.is_empty());
    assert!(workspace.is_visible(PanelKind::ScriptPanel(77)));
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let element = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(element.rect[0], element.rect[1]), vec2(element.rect[2], element.rect[3]))
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2, to: egui::Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for fraction in [0.05, 0.25, 0.5, 0.75, 1.0] {
        h.event(Event::PointerMoved(from.lerp(to, fraction)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

#[test]
fn pointer_drop_merges_floats_and_redocks_without_project_changes() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Docking composition", "width": 320, "height": 180, "duration": 4})).unwrap();
    session.execute("layer.newSolid", json!({"name": "Teal plate", "width": 320, "height": 180, "color": "#245b68"})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.start_screen = false;
    app.ui.dock = dock::docked(dock::vsplit(
        SplitSize::Ratio(0.62),
        dock::hsplit(
            SplitSize::FixedFirst(400.0),
            dock::tabs(&[PanelKind::Project, PanelKind::Info, PanelKind::Audio], 1),
            dock::tabs(&[PanelKind::Composition, PanelKind::Layer], 0),
        ),
        dock::tabs(&[PanelKind::Timeline], 0),
    ));
    let mut h = Harness::builder().with_size(vec2(1200.0, 760.0)).build_eframe(move |_| app);
    h.run_steps(4);
    let project = h.state().session.project.clone();
    let times = h.state().session.state.times.clone();
    // A tab dropped on another group's tab strip joins it.
    let from = rect(&h, "panel.tab.Info").center();
    let to = rect(&h, "panel.tab.Layer").center() + vec2(20.0, 0.0);
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().ui.dock.is_visible(PanelKind::Info));
    assert!(h.state().ui.dock.group_of(PanelKind::Info).contains(&PanelKind::Composition));
    // Cmd/Ctrl at the release floats it.
    let from = rect(&h, "panel.tab.Info").center();
    drag(&mut h, from, pos2(700.0, 400.0), Modifiers::COMMAND);
    assert_eq!(floating(h.state()).len(), 1);
    assert!(h.state().ui.dock.is_floating(PanelKind::Info));
    let grips: Vec<_> = h.query_all_by_label("Resize panel window").collect();
    assert!(!grips.is_empty() && grips.iter().all(|grip| grip.accesskit_node().role() == egui::accesskit::Role::Button));
    // Its corner resizes it and its title bar moves it.
    let original = floating(h.state())[0].rect;
    let grip = rect(&h, "panel.float.0.resize").center();
    drag(&mut h, grip, grip - vec2(36.0, 24.0), Modifiers::NONE);
    let resized = floating(h.state())[0].rect;
    assert!(resized[2] < original[2] - 25.0 && resized[3] < original[3] - 15.0, "{original:?} -> {resized:?}");
    let bar = rect(&h, "panel.float.0.move");
    drag(&mut h, bar.center(), bar.center() - vec2(24.0, 18.0), Modifiers::NONE);
    let moved = floating(h.state())[0].rect;
    assert!(moved[0] < resized[0] - 15.0 && moved[1] < resized[1] - 10.0, "{resized:?} -> {moved:?}");
    // Its tab dropped on a docked tab strip docks it again.
    let from = rect(&h, "panel.tab.Info").center();
    let to = rect(&h, "panel.tab.Project").center() + vec2(20.0, 0.0);
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(floating(h.state()).is_empty());
    assert!(h.state().ui.dock.group_of(PanelKind::Info).contains(&PanelKind::Project));
    assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
    assert_eq!(times, h.state().session.state.times);
}

#[test]
fn current_and_named_floating_layouts_survive_restart() {
    use effectcraft_engine::config::MemoryConfig;
    use std::sync::Arc;
    let store = Arc::new(MemoryConfig::default());
    let mut app = EffectcraftApp::new(Session { config: Some(store.clone()), ..Default::default() });
    app.apply_dock(Action::Float { panel: PanelKind::Project, rect: [45.0, 65.0, 430.0, 310.0] }).unwrap();
    invoke(&mut app, "window.saveWorkspaceAs", json!({"name": "Shared float"})).unwrap();
    let saved = app.ui.dock.clone();
    app.apply_dock(Action::MoveFloating { panel: PanelKind::Project, rect: [75.0, 95.0, 480.0, 360.0] }).unwrap();
    let current = app.ui.dock.clone();
    eframe::App::on_exit(&mut app);
    let mut restarted = EffectcraftApp::new(Session { config: Some(store), ..Default::default() });
    assert_eq!(restarted.ui.dock, current);
    restarted.set_workspace("Default");
    restarted.set_workspace("Shared float");
    assert_eq!(restarted.ui.dock, saved);
}

#[test]
fn float_command_rejects_malformed_rectangles_and_keeps_one_docked_panel() {
    let mut app = EffectcraftApp::new(Session::default());
    let before = app.ui.dock.clone();
    for rect in [json!([1, 2, 300]), json!([1, 2, 300, 200, 7]), json!([1, 2, 1e100, 200]), json!("rectangle")] {
        assert!(invoke(&mut app, "window.floatPanel", json!({"panel": "Project", "rect": rect})).is_err());
        assert_eq!(app.ui.dock, before);
    }
    // The last docked panel stays docked, whether it moves alone, with its group or by command.
    app.ui.dock = dock::docked(dock::tabs(&[PanelKind::Project], 0));
    let before = app.ui.dock.clone();
    assert!(app.apply_dock(Action::Float { panel: PanelKind::Project, rect: [1.0, 2.0, 300.0, 200.0] }).is_err());
    assert!(invoke(&mut app, "window.panel.float", json!({"panel": "Project", "group": true})).is_err());
    assert_eq!(app.ui.dock, before);
    // Closing it is allowed.
    invoke(&mut app, "window.closePanel", json!({"panel": "Project"})).unwrap();
    assert!(app.ui.dock.layout.root.is_none());
}

/// A floating panel dragged towards the window's bottom edge moves there, and its title bar
/// still moves it from there.
#[test]
fn floating_panel_dragged_towards_the_window_edge_keeps_moving() {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| {
        let mut app = EffectcraftApp::new(Session::default());
        app.ui.start_screen = false;
        app
    });
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.floatPanel", json!({"panel": "Preview", "rect": [600.0, 400.0, 360.0, 300.0]})).unwrap();
    h.run_steps(3);
    let bar = rect(&h, "panel.float.0.move");
    drag(&mut h, bar.center(), bar.center() + vec2(40.0, 420.0), Modifiers::NONE);
    let moved = floating(h.state())[0].rect;
    assert!(moved[1] > 550.0, "the panel moved down: {moved:?}");
    let bar = rect(&h, "panel.float.0.move");
    drag(&mut h, bar.center(), bar.center() - vec2(30.0, 200.0), Modifiers::NONE);
    let back = floating(h.state())[0].rect;
    assert!(back[1] < moved[1] - 150.0, "{moved:?} -> {back:?}");
}

#[test]
fn catalog_docking_action_supports_resize_reorder_and_floating_geometry() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = dock::docked(dock::hsplit(
        SplitSize::FixedFirst(300.0),
        dock::tabs(&[PanelKind::Project, PanelKind::Info], 1),
        dock::tabs(&[PanelKind::Composition], 0),
    ));
    for action in [
        Action::ResizeSplit { path: vec![], size: SplitSize::FixedFirst(360.0) },
        Action::Move { panel: PanelKind::Info, anchor: PanelKind::Project, placement: Placement::Tab { before: Some(PanelKind::Project) } },
        Action::Float { panel: PanelKind::Info, rect: [20.0, 40.0, 300.0, 200.0] },
        Action::MoveFloating { panel: PanelKind::Info, rect: [45.0, 65.0, 330.0, 240.0] },
    ] {
        invoke(&mut app, "window.dockAction", json!({"action": action})).unwrap();
    }
    assert_eq!(floating(&app)[0].rect, [45.0, 65.0, 330.0, 240.0]);
    let Some(Node::Split { size, .. }) = &app.ui.dock.layout.root else { panic!("split lost") };
    assert_eq!(*size, SplitSize::FixedFirst(360.0));
    let before = app.ui.dock.clone();
    assert!(invoke(&mut app, "window.dockAction", json!({"action": {"ResizeSplit": {"path": [true, true], "size": {"Ratio": 0.5}}}})).is_err());
    assert!(invoke(&mut app, "window.dockAction", json!({"action": {"Open": {"panel": "Align", "anchor": null}}})).is_err());
    assert_eq!(app.ui.dock, before);
}

#[test]
fn ui_dock_and_window_panel_commands_parse_craft_ui_commands() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = dock::docked(dock::hsplit(
        SplitSize::FixedFirst(300.0),
        dock::tabs(&[PanelKind::Project, PanelKind::Info], 0),
        dock::tabs(&[PanelKind::Composition, PanelKind::Layer], 0),
    ));
    // `ui.dock` operations and actions return the workspace.
    let reply = invoke(&mut app, "ui.dock", json!({"operation": "move", "panel": "Info", "target": "Composition", "zone": "center"})).unwrap();
    assert!(reply["docking"]["layout"].is_object(), "{reply}");
    assert!(app.ui.dock.group_of(PanelKind::Info).contains(&PanelKind::Composition));
    invoke(&mut app, "ui.dock", json!({"action": {"Activate": {"panel": "Layer"}}})).unwrap();
    assert!(app.ui.dock.is_visible(PanelKind::Layer));
    invoke(&mut app, "ui.dock", json!({"operation": "float", "panel": "Info", "rect": [100, 100, 300, 200]})).unwrap();
    assert_eq!(floating(&app)[0].rect, [100.0, 100.0, 300.0, 200.0]);
    // `window.panel.*` takes EffectCraft's panel names too.
    invoke(&mut app, "window.panel.dock", json!({"panel": "Info", "anchor": "Project", "zone": "right"})).unwrap();
    assert!(floating(&app).is_empty() && app.ui.dock.is_docked(PanelKind::Info));
    invoke(&mut app, "window.panel.close", json!({"panel": "Info"})).unwrap();
    assert!(!app.ui.dock.is_open(PanelKind::Info));
    // A closed panel reopens where it was.
    invoke(&mut app, "window.panel.activate", json!({"panel": "Info"})).unwrap();
    assert!(app.ui.dock.is_docked(PanelKind::Info));
    // The per-panel ids still open a panel.
    invoke(&mut app, "window.panel.Align", json!({})).unwrap();
    assert!(app.ui.dock.is_open(PanelKind::Align));
    // Bad commands change nothing; Reset returns to the workspace's layout.
    let before = app.ui.dock.clone();
    for (id, params) in [
        ("ui.dock", json!({"operation": "move", "panel": "Info", "target": "Nope"})),
        ("ui.dock", json!({"operation": "teleport", "panel": "Info"})),
        ("window.panel.move", json!({"panel": "Info"})),
        ("window.panel.dock", json!({"panel": "Nope", "anchor": "Project"})),
    ] {
        assert!(invoke(&mut app, id, params.clone()).is_err(), "{id} {params}");
        assert_eq!(app.ui.dock, before);
    }
    invoke(&mut app, "ui.dock.reset", json!({})).unwrap();
    assert_eq!(app.ui.dock, Workspace::new(dock::workspace("Default")));
}

#[test]
fn catalog_close_keeps_dynamic_panel_identity_and_releases_viewers() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::Viewer(42), PanelKind::ScriptPanel(77)], 1));
    app.ui.viewers.insert(42, Some(7));
    app.ui.maximized = Some(PanelKind::Viewer(42));
    for panel in [PanelKind::Viewer(42), PanelKind::ScriptPanel(77)] {
        invoke(&mut app, "window.dockAction", json!({"action": Action::Close { panel }})).unwrap();
        assert!(!app.ui.dock.is_open(panel));
        assert!(app.ui.dock.hidden.iter().any(|(p, _)| *p == panel), "{panel:?} keeps its place");
    }
    assert!(app.ui.dock.is_docked(PanelKind::Composition));
    assert_eq!(app.ui.maximized, None);
    assert!(!app.ui.viewers.get(&42).is_some_and(Option::is_some), "the closed viewer forgets its comp");
}

#[test]
fn stacked_panels_resize_by_pointer_without_changing_closed_entries() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width": 160, "height": 90, "duration": 1})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.start_screen = false;
    let mut h = Harness::builder().with_size(vec2(1000.0, 720.0)).build_eframe(move |_| app);
    h.run_steps(4);
    h.state_mut().ui.dock =
        dock::docked(dock::stack(&[(PanelKind::Preview, true, Some(100.0)), (PanelKind::Properties, true, None), (PanelKind::Align, false, Some(77.0))]));
    h.run_steps(3);
    let project = h.state().session.project.clone();
    let before = rect(&h, "panel.Preview").height();
    let from = rect(&h, "dock.gutter.s0").center();
    drag(&mut h, from, from + vec2(0.0, 16.0), Modifiers::NONE);
    let after = rect(&h, "panel.Preview").height();
    assert!((after - before - 16.0).abs() < 2.0, "{before} -> {after}");
    let Some(Node::Stack { entries }) = &h.state().ui.dock.layout.root else { panic!("stack lost") };
    assert!(entries[0].height.unwrap_or_default() > 100.0, "{entries:?}");
    assert_eq!(entries[2], StackEntry { panel: PanelKind::Align, open: false, height: Some(77.0) });
    assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
}

#[test]
fn failed_close_preserves_viewer_lifecycle_and_maximize_state() {
    let mut app = EffectcraftApp::new(Session::default());
    // A locked workspace refuses the close.
    app.ui.dock = dock::docked(dock::tabs(&[PanelKind::Composition, PanelKind::Viewer(42)], 1));
    app.ui.dock.locked = true;
    app.ui.maximized = Some(PanelKind::Viewer(42));
    app.ui.viewers.insert(42, Some(3));
    app.ui.active_viewer = 42;
    app.ui.locked_tabs.insert(PanelKind::Viewer(42).id());
    let before = app.ui.clone();
    app.close_panel(PanelKind::Viewer(42));
    assert_eq!(app.ui.dock, before.dock);
    assert_eq!(app.ui.maximized, before.maximized);
    assert_eq!(app.ui.viewers, before.viewers);
    assert_eq!(app.ui.active_viewer, before.active_viewer);
    assert_eq!(app.ui.locked_tabs, before.locked_tabs);
}

/// A panel never laid out opens at EffectCraft's default place for it (History beside the
/// Project panel, Render Queue with the Timeline), not where craft-ui's fallback puts it; a panel
/// the user closed comes back where it was.
#[test]
fn never_placed_panels_open_at_their_default_place() {
    let mut app = EffectcraftApp::new(Session::default());
    assert!(!app.ui.dock.is_open(PanelKind::History));
    invoke(&mut app, "window.panel", json!({"panel": "History"})).unwrap();
    assert!(app.ui.dock.group_of(PanelKind::History).contains(&PanelKind::Project), "{:?}", app.ui.dock.group_of(PanelKind::History));
    invoke(&mut app, "window.panel.activate", json!({"panel": "Tracker"})).unwrap();
    assert!(app.ui.dock.group_of(PanelKind::Tracker).contains(&PanelKind::EffectsPresets) || app.ui.dock.is_open(PanelKind::Tracker));
    // Closed by the user, it reopens at its recorded place.
    invoke(&mut app, "window.dockPanel", json!({"panel": "History", "anchor": "Timeline"})).unwrap();
    invoke(&mut app, "window.closePanel", json!({"panel": "History"})).unwrap();
    invoke(&mut app, "window.panel", json!({"panel": "History"})).unwrap();
    assert!(app.ui.dock.group_of(PanelKind::History).contains(&PanelKind::Timeline));
}
