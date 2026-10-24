//! Shared pane resizing in the full application, including application shortcut ownership.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{DockNode, PanelKind, SplitSize};
use effectcraft_ui_egui::theme::ThemeKind;
use egui::{Event, Id, Key, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn snapshots(h: &mut Harness<'_, EffectcraftApp>, directory: Option<&std::path::Path>, phase: &str) {
    let Some(directory) = directory else { return };
    std::fs::create_dir_all(directory).unwrap();
    for (name, theme) in [("dark", ThemeKind::Dark), ("darker", ThemeKind::Darker), ("light", ThemeKind::Light)] {
        let ctx = h.ctx.clone();
        h.state_mut().set_theme(&ctx, theme);
        h.run_steps(3);
        h.render().unwrap().save(directory.join(format!("effectcraft-{phase}-{name}.png"))).unwrap();
    }
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, ThemeKind::Dark);
    h.run_steps(3);
}

fn first_size(h: &Harness<'_, EffectcraftApp>) -> f32 {
    match h.state().ui.dock {
        DockNode::Split { size: SplitSize::FixedA(value), .. } => value,
        ref other => panic!("fixed-size mode changed: {other:?}"),
    }
}

#[test]
fn focused_dock_resize_owns_arrows_and_keeps_pointer_dragging() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Split test", "width": 160, "height": 90, "duration": 1})).unwrap();
    session.execute("layer.newSolid", json!({"name": "Selected", "color": [1.0, 0.0, 0.0, 1.0]})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.start_screen = false;
    let directory = std::env::var_os("CRAFT_UI_PANES_SNAPSHOT_DIR").map(std::path::PathBuf::from);
    let mut builder = Harness::builder().with_size(vec2(1400.0, 900.0)).with_max_steps(10_000);
    if directory.is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_| app);
    h.run_steps(4);
    snapshots(&mut h, directory.as_deref(), "workspace");
    h.state_mut().ui.focused = PanelKind::Composition;
    h.state_mut().ui.dock = DockNode::Split {
        vertical: false,
        size: SplitSize::FixedA(400.0),
        a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 }),
        b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Composition], active: 0 }),
    };
    h.run_steps(3);
    let before_project = h.state().session.project.clone();
    let before_times = h.state().session.state.times.clone();
    let id = Id::new(("dock-gutter", String::new()));
    h.ctx.memory_mut(|memory| memory.request_focus(id));
    h.run_steps(2);
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(id));
    snapshots(&mut h, directory.as_deref(), "focused-resize");
    h.event(Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::Key { key: Key::ArrowRight, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(first_size(&h), 408.0);
    assert_eq!(h.state().session.state.times, before_times);
    assert!(std::sync::Arc::ptr_eq(&h.state().session.project, &before_project), "the viewer shortcut must not nudge a selected layer");

    h.run_steps(2);
    let element = h.state().auto.find("dock.gutter.").unwrap();
    let from = Rect::from_min_size(pos2(element.rect[0], element.rect[1]), vec2(element.rect[2], element.rect[3])).center();
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for offset in [5.0, 10.0, 15.0, 20.0] {
        h.event(Event::PointerMoved(from + vec2(offset, 0.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: from + vec2(20.0, 0.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(first_size(&h), 428.0);
    assert_eq!(h.state().session.state.times, before_times);
    assert!(std::sync::Arc::ptr_eq(&h.state().session.project, &before_project));
}
