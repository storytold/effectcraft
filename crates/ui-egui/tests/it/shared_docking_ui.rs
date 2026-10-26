//! Existing EffectCraft workspaces use shared transactions without changing their saved schema.
use craft_ui::docking::{Action, Placement, Zone};
use effectcraft_engine::Session;
use effectcraft_ui_egui::{
    EffectcraftApp,
    dock::{self, DockNode, Layout, PanelKind, SplitSize, StackEntry},
};
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

#[test]
fn shared_move_preserves_accordion_metadata_and_unrelated_active_tabs() {
    let mut layout = Layout {
        root: DockNode::Split {
            vertical: false,
            size: SplitSize::FixedB(300.0),
            a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Composition, PanelKind::Layer], active: 1 }),
            b: Box::new(DockNode::Stack {
                entries: vec![
                    StackEntry::new(PanelKind::Preview, false, Some(73.0)),
                    StackEntry::new(PanelKind::Properties, true, Some(211.0)),
                    StackEntry::new(PanelKind::Align, false, None),
                ],
            }),
        },
        floating: vec![],
    };
    layout.apply(Action::Close { panel: PanelKind::Project }).unwrap();
    assert!(layout.root.is_visible(PanelKind::Composition));
    layout.apply(Action::Move { panel: PanelKind::Preview, anchor: PanelKind::Align, placement: Placement::Tab { before: None } }).unwrap();
    let DockNode::Split { b, size, .. } = &layout.root else { panic!("split was lost") };
    assert_eq!(*size, SplitSize::FixedB(300.0));
    let DockNode::Stack { entries } = b.as_ref() else { panic!("stack was lost") };
    // Upstream now makes a tab drop join the anchor's accordion frame. Its height belongs
    // to the destination frame; the unrelated Properties frame keeps its exact metadata.
    let joined = entries.iter().find(|entry| entry.contains(PanelKind::Preview)).unwrap();
    assert_eq!(joined.panels, vec![PanelKind::Align, PanelKind::Preview]);
    assert_eq!(joined.shown(), Some(PanelKind::Preview));
    assert!(joined.open);
    assert_eq!(joined.height, None);
    assert_eq!(entries.iter().find(|entry| entry.contains(PanelKind::Properties)).unwrap(), &StackEntry::new(PanelKind::Properties, true, Some(211.0)));
    let before = layout.clone();
    assert!(layout.apply(Action::Move { panel: PanelKind::Preview, anchor: PanelKind::Audio, placement: Placement::Split(Zone::Left) }).is_err());
    assert_eq!(layout, before);
    assert!(layout.apply(Action::Float { panel: PanelKind::Preview, rect: [0.0, 0.0, f32::INFINITY, 200.0] }).is_err());
    assert_eq!(layout, before);
}

#[test]
fn every_builtin_workspace_and_dynamic_identity_roundtrips() {
    for name in dock::WORKSPACES {
        let mut layout = Layout { root: dock::workspace(name), floating: dock::workspace_floating(name) };
        let before = serde_json::to_value(&layout).unwrap();
        // A structural no-op still crosses both adapters and shared validation. The complete
        // persisted shape must remain byte-equivalent, including accordion state and heights.
        let action = match &layout.root {
            DockNode::Split { size, .. } => Action::ResizeSplit {
                path: vec![],
                size: match *size {
                    SplitSize::Ratio(v) => craft_ui::layout::SplitSize::Ratio(v),
                    SplitSize::FixedA(v) => craft_ui::layout::SplitSize::FixedFirst(v),
                    SplitSize::FixedB(v) => craft_ui::layout::SplitSize::FixedSecond(v),
                },
            },
            DockNode::Tabs { panels, .. } => Action::Move { panel: panels[0], anchor: panels[1], placement: Placement::Tab { before: Some(panels[0]) } },
            DockNode::Stack { entries } => Action::SetStackOpen { panel: entries[0].shown().unwrap(), open: entries[0].open },
        };
        layout.apply(action).unwrap();
        assert_eq!(before, serde_json::to_value(&layout).unwrap());
        let back: Layout = serde_json::from_value(before).unwrap();
        assert_eq!(layout, back);
    }
    let mut layout = Layout {
        root: DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Viewer(42), PanelKind::ScriptPanel(77)], active: 1 },
        floating: vec![],
    };
    layout.apply(Action::Float { panel: PanelKind::ScriptPanel(77), rect: [20.0, 30.0, 400.0, 300.0] }).unwrap();
    layout.apply(Action::Move { panel: PanelKind::ScriptPanel(77), anchor: PanelKind::Viewer(42), placement: Placement::Tab { before: None } }).unwrap();
    assert!(layout.floating.is_empty());
    assert!(layout.contains(PanelKind::Viewer(42)));
    assert!(layout.root.is_visible(PanelKind::ScriptPanel(77)));
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
    session.execute("layer.newSolid", json!({"name": "Coral tile", "width": 110, "height": 70, "color": "#ee7859"})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.start_screen = false;
    app.ui.dock = DockNode::Split {
        vertical: false,
        size: SplitSize::FixedA(400.0),
        a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Info, PanelKind::Audio], active: 1 }),
        b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Layer], active: 0 }),
    };
    app.ui.dock = DockNode::Split {
        vertical: true,
        size: SplitSize::Ratio(0.62),
        a: Box::new(app.ui.dock),
        b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Timeline], active: 0 }),
    };
    let snapshots = std::env::var_os("CRAFT_UI_DOCKING_SNAPSHOT_DIR").map(std::path::PathBuf::from);
    let mut builder = Harness::builder().with_size(vec2(1200.0, 760.0));
    if snapshots.is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_| app);
    h.run_steps(4);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while h.state().frames.urgent_pending() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!h.state().frames.urgent_pending());
    h.run_steps(3);
    let project = h.state().session.project.clone();
    let times = h.state().session.state.times.clone();
    let mut action_log = vec![action_state(&h, "start")];
    let from = rect(&h, "panel.tab.Info").center();
    let to = rect(&h, "panel.Composition").center();
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().ui.dock.is_visible(PanelKind::Info));
    assert!(same_group(&h.state().ui.dock, PanelKind::Info, PanelKind::Composition));
    action_log.push(action_state(&h, "merged"));
    let from = rect(&h, "panel.tab.Info").center();
    let before = rect(&h, "panel.tab.Composition");
    drag(&mut h, from, pos2(before.left() + 2.0, before.center().y), Modifiers::NONE);
    assert_eq!(tabs_containing(&h.state().ui.dock, PanelKind::Info).first(), Some(&PanelKind::Info));
    action_log.push(action_state(&h, "reordered"));

    let from = rect(&h, "panel.tab.Info").center();
    drag(&mut h, from, pos2(700.0, 400.0), Modifiers::COMMAND);
    assert_eq!(h.state().ui.floating.len(), 1);
    // The grip is a resize handle: the migration's role mapping (`ResizeHandle → Splitter`) applies
    // to floating panels too, and the widget reports it (`dock_ui.rs`).
    assert_eq!(h.get_by_label("Resize floating panel").accesskit_node().role(), egui::accesskit::Role::Splitter);
    action_log.push(action_state(&h, "floated"));
    let original = h.state().ui.floating[0].rect;
    let grip = rect(&h, "panel.float.0.resize").center();
    drag(&mut h, grip, grip + vec2(36.0, 24.0), Modifiers::NONE);
    assert!(h.state().ui.floating[0].rect[2] > original[2] + 25.0);
    assert!(h.state().ui.floating[0].rect[3] > original[3] + 15.0);
    action_log.push(action_state(&h, "resized"));
    let grip_id = egui::Id::unique(("floating-panel", PanelKind::Info.id())).with("resize");
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(grip_id));
    let before_keys = h.state().ui.floating[0].rect;
    for key in [egui::Key::ArrowRight, egui::Key::ArrowDown] {
        for pressed in [true, false] {
            h.event(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
            h.step();
        }
    }
    assert_eq!(h.state().ui.floating[0].rect[2], before_keys[2] + 8.0);
    assert_eq!(h.state().ui.floating[0].rect[3], before_keys[3] + 8.0);
    h.event(Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::Key { key: egui::Key::Tab, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_ne!(h.ctx.memory(|memory| memory.focused()), Some(grip_id));
    action_log.push(action_state(&h, "keyboard-resized"));
    let resized = h.state().ui.floating[0].rect;
    let header = rect(&h, "panel.float.0.move");
    let from = pos2(header.right() - 24.0, header.center().y);
    drag(&mut h, from, from - vec2(24.0, 18.0), Modifiers::NONE);
    assert!(h.state().ui.floating[0].rect[0] < resized[0] - 15.0);
    assert!(h.state().ui.floating[0].rect[1] < resized[1] - 10.0);
    action_log.push(action_state(&h, "moved"));

    if let Some(directory) = &snapshots {
        std::fs::create_dir_all(directory).unwrap();
        h.render().unwrap().save(directory.join("effectcraft-floating.png")).unwrap();
    }
    let from = rect(&h, "panel.tab.Info").center();
    let to = rect(&h, "panel.Audio").center();
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().ui.floating.is_empty());
    action_log.push(action_state(&h, "redocked"));
    assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
    assert_eq!(times, h.state().session.state.times);
    assert!(h.state().auto.elements.iter().any(|element| element.id.starts_with("timeline.layer.")));
    h.state_mut().session.execute("view.exposure", json!({"stops": 2.0})).unwrap();
    h.run_steps(3);
    let pos = rect(&h, "viewer.resetExposure").center();
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert_eq!(h.state().session.state.viewer.exposure, 0.0);
    action_log.push(action_state(&h, "viewer-reset-exposure"));
    if let Some(directory) = &snapshots {
        std::fs::write(directory.join("effectcraft-actions.json"), serde_json::to_vec_pretty(&action_log).unwrap()).unwrap();
    }
    if let Some(directory) = &snapshots {
        h.render().unwrap().save(directory.join("effectcraft-redocked.png")).unwrap();
    }
    assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
}

fn same_group(root: &DockNode, first: PanelKind, second: PanelKind) -> bool {
    match root {
        DockNode::Tabs { panels, .. } => panels.contains(&first) && panels.contains(&second),
        DockNode::Split { a, b, .. } => same_group(a, first, second) || same_group(b, first, second),
        DockNode::Stack { .. } => false,
    }
}

#[test]
fn current_and_named_floating_layouts_survive_restart() {
    use effectcraft_engine::config::MemoryConfig;
    use std::sync::Arc;
    let store = Arc::new(MemoryConfig::default());
    let mut app = EffectcraftApp::new(Session { config: Some(store.clone()), ..Default::default() });
    assert!(app.edit_layout(|layout| layout.apply(Action::Float { panel: PanelKind::Project, rect: [45.0, 65.0, 430.0, 310.0] }).is_ok()));
    let ctx = egui::Context::default();
    effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "window.saveWorkspaceAs", json!({"name": "Shared float"})).unwrap();
    let saved = app.ui.floating.clone();
    assert!(app.edit_layout(|layout| layout.apply(Action::MoveFloating { panel: PanelKind::Project, rect: [75.0, 95.0, 480.0, 360.0] }).is_ok()));
    let current = app.ui.floating.clone();
    eframe::App::on_exit(&mut app);
    let mut restarted = EffectcraftApp::new(Session { config: Some(store), ..Default::default() });
    assert_eq!(restarted.ui.floating, current);
    restarted.set_workspace("Default");
    restarted.set_workspace("Shared float");
    assert_eq!(restarted.ui.floating, saved);
}

fn tabs_containing(root: &DockNode, panel: PanelKind) -> Vec<PanelKind> {
    match root {
        DockNode::Tabs { panels, .. } if panels.contains(&panel) => panels.clone(),
        DockNode::Split { a, b, .. } => {
            let first = tabs_containing(a, panel);
            if first.is_empty() { tabs_containing(b, panel) } else { first }
        }
        _ => vec![],
    }
}

#[test]
fn float_command_rejects_malformed_rectangles_atomically() {
    let mut app = EffectcraftApp::new(Session::default());
    let before = app.ui.dock.clone();
    let ctx = egui::Context::default();
    for rect in [json!([1, 2, 300]), json!([1, 2, 300, 200, 7]), json!([1, 2, 1e100, 200]), json!("rectangle")] {
        assert!(effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": "Project", "rect": rect})).is_err());
        assert_eq!(app.ui.dock, before);
        assert!(app.ui.floating.is_empty());
    }
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 };
    assert!(!app.edit_layout(|layout| layout.apply(Action::Float { panel: PanelKind::Project, rect: [1.0, 2.0, 300.0, 200.0] }).is_ok()));
}

/// A floating panel dragged past the window's bottom edge is drawn where its rect says: it was
/// shifted back into the window while its rect kept moving, and its strip then clashed with the
/// floating area's own widget ("Double use of widget ID").
#[test]
fn floating_panel_dragged_past_the_window_edge_is_drawn_at_its_rect() {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.floatPanel", json!({"panel": "Preview", "rect": [600.0, 400.0, 360.0, 300.0]})).unwrap();
    h.run_steps(3);
    let strip = rect(&h, "panel.float.0.move");
    let from = pos2(strip.right() - 24.0, strip.center().y);
    drag(&mut h, from, from + vec2(40.0, 420.0), Modifiers::NONE);
    let moved = h.state().ui.floating[0].rect;
    assert!(moved[1] + moved[3] > 1000.0, "the panel hangs past the bottom edge: {moved:?}");
    let strip = rect(&h, "panel.float.0.move");
    assert_eq!([strip.min.x, strip.min.y], [moved[0], moved[1]]);
    // The strip still moves it from there.
    let from = pos2(strip.right() - 24.0, strip.center().y);
    drag(&mut h, from, from - vec2(30.0, 200.0), Modifiers::NONE);
    assert_eq!(h.state().ui.floating[0].rect[1], moved[1] - 200.0);
}

fn action_state(h: &Harness<'_, EffectcraftApp>, action: &str) -> serde_json::Value {
    json!({"action": action, "dock": h.state().ui.dock, "floating": h.state().ui.floating, "focused": h.state().ui.focused})
}

#[test]
fn catalog_docking_action_supports_resize_reorder_and_floating_geometry() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = DockNode::Split {
        vertical: false,
        size: SplitSize::FixedA(300.0),
        a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Info], active: 1 }),
        b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Composition], active: 0 }),
    };
    let ctx = egui::Context::default();
    for action in [
        Action::ResizeSplit { path: vec![], size: craft_ui::layout::SplitSize::FixedFirst(360.0) },
        Action::Move { panel: PanelKind::Info, anchor: PanelKind::Project, placement: Placement::Tab { before: Some(PanelKind::Project) } },
        Action::Float { panel: PanelKind::Info, rect: [20.0, 40.0, 300.0, 200.0] },
        Action::MoveFloating { panel: PanelKind::Info, rect: [45.0, 65.0, 330.0, 240.0] },
    ] {
        effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "window.dockAction", json!({"action": action})).unwrap();
    }
    assert_eq!(app.ui.floating[0].rect, [45.0, 65.0, 330.0, 240.0]);
    let DockNode::Split { size, .. } = &app.ui.dock else { panic!("split lost") };
    assert_eq!(*size, SplitSize::FixedA(360.0));
    let before = json!({"dock": app.ui.dock, "floating": app.ui.floating});
    assert!(
        effectcraft_ui_egui::menus::invoke(
            &mut app,
            &ctx,
            "window.dockAction",
            json!({"action": {
                "ResizeSplit": {"path": [true, true], "size": {"Ratio": 0.5}}
            }})
        )
        .is_err()
    );
    assert_eq!(json!({"dock": app.ui.dock, "floating": app.ui.floating}), before);
}

#[test]
fn catalog_close_keeps_dynamic_panel_identity() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Viewer(42), PanelKind::ScriptPanel(77)], active: 1 };
    let ctx = egui::Context::default();
    for panel in [PanelKind::Viewer(42), PanelKind::ScriptPanel(77)] {
        effectcraft_ui_egui::menus::invoke(&mut app, &ctx, "window.dockAction", json!({"action": Action::Close { panel }})).unwrap();
        assert!(!app.ui.dock.contains(panel));
    }
    assert!(app.ui.dock.contains(PanelKind::Composition));
}

#[test]
fn accordion_resize_routes_pointer_and_keyboard_without_changing_closed_entries() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width": 160, "height": 90, "duration": 1})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.start_screen = false;
    let mut h = Harness::builder().with_size(vec2(1000.0, 720.0)).build_eframe(move |_| app);
    h.run_steps(4);
    h.state_mut().ui.dock = DockNode::Stack {
        entries: vec![
            StackEntry::new(PanelKind::Preview, true, Some(100.0)),
            StackEntry::new(PanelKind::Properties, true, None),
            StackEntry::new(PanelKind::Align, false, Some(77.0)),
        ],
    };
    h.run_steps(3);
    let project = h.state().session.project.clone();
    let id = egui::Id::unique(("dock-gutter", "s0".to_string()));
    h.ctx.memory_mut(|memory| memory.request_focus(id));
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(Event::Key { key: egui::Key::ArrowDown, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    let DockNode::Stack { entries } = &h.state().ui.dock else { panic!("stack lost") };
    assert_eq!(entries[0].height, Some(108.0));
    let from = rect(&h, "dock.gutter.s0").center();
    drag(&mut h, from, from + vec2(0.0, 16.0), Modifiers::NONE);
    let DockNode::Stack { entries } = &h.state().ui.dock else { panic!("stack lost") };
    assert_eq!(entries[0].height, Some(124.0));
    assert_eq!(entries[1].height, None);
    assert_eq!(entries[2], StackEntry::new(PanelKind::Align, false, Some(77.0)));
    assert!(std::sync::Arc::ptr_eq(&project, &h.state().session.project));
}

#[test]
fn legacy_close_repairs_selection_but_transactions_remain_strict() {
    use effectcraft_ui_egui::dock::Floating;
    let mut root = DockNode::Tabs { panels: vec![PanelKind::Properties, PanelKind::Align, PanelKind::EffectsPresets], active: usize::MAX };
    root.close(PanelKind::History);
    assert!(root.is_visible(PanelKind::EffectsPresets));
    let mut layout = Layout {
        root: DockNode::Tabs { panels: vec![PanelKind::Composition], active: 0 },
        floating: vec![Floating {
            panels: vec![PanelKind::Properties, PanelKind::Align, PanelKind::EffectsPresets],
            active: usize::MAX,
            rect: [0.0, 0.0, 300.0, 200.0],
        }],
    };
    let before = layout.clone();
    assert!(layout.apply(Action::Close { panel: PanelKind::Properties }).is_err());
    assert_eq!(layout, before);
    assert!(layout.unfloat(PanelKind::Properties));
    assert_eq!(layout.floating[0].panels.get(layout.floating[0].active), Some(&PanelKind::EffectsPresets));
    layout.floating[0].rect = [0.0; 4];
    let before = layout.clone();
    assert!(!layout.unfloat(PanelKind::Align));
    assert_eq!(layout, before);
}

#[test]
fn failed_close_preserves_viewer_lifecycle_and_maximize_state() {
    let mut app = EffectcraftApp::new(Session::default());
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Viewer(42)], active: usize::MAX };
    app.ui.maximized = Some(PanelKind::Viewer(42));
    app.ui.viewers.insert(42, None);
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
