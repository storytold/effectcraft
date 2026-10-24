//! #753: the Layer panel zooms and pans (magnification menu, View ▸ Zoom In / Zoom Out while it
//! is focused, the wheel, the Zoom and Hand tools), and the Roto Brush explains itself in the
//! Composition panel (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::state::Tool;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "View", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    EffectcraftApp::new(s)
}

fn box_id(h: &Harness<'_, EffectcraftApp>) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id.0
}

/// The harness with Box open in the Layer panel (focused).
fn harness() -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.run_steps(3);
    let id = box_id(&h);
    h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.openLayer", json!({"layer": id})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().ui.focused, PanelKind::Layer);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn press(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, button: egui::PointerButton, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed, modifiers: Default::default() });
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    press(h, p, egui::PointerButton::Primary, true);
    h.step();
    press(h, p, egui::PointerButton::Primary, false);
    h.run_steps(2);
}

fn drag_with(h: &mut Harness<'_, EffectcraftApp>, button: egui::PointerButton, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    press(h, from, button, true);
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    press(h, to, button, false);
    h.run_steps(2);
}

fn command(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, json!({})).unwrap();
    h.run_steps(3);
}

/// Shown magnification: the canvas width per layer pixel (Box is 80 px wide).
fn shown(h: &Harness<'_, EffectcraftApp>) -> f32 {
    rect(h, "layerPanel.canvas").width() / 80.0
}

#[test]
fn zoom_commands_act_on_the_focused_layer_panel() {
    let mut h = harness();
    assert!(h.state().auto.find("layerPanel.magnification").is_some(), "the Layer panel has a magnification menu");
    let fit = shown(&h);
    let comp_zoom = h.state().ui.viewer.zoom;
    command(&mut h, "view.zoomOut");
    let z = h.state().ui.layer_zoom.expect("Zoom Out sets the Layer panel's magnification");
    assert!(z < fit, "{z} < {fit}");
    assert!((shown(&h) - z).abs() < 0.01, "the canvas is drawn at the new magnification");
    assert_eq!(h.state().ui.viewer.zoom, comp_zoom, "the Composition viewer keeps its magnification");
    command(&mut h, "view.zoomIn");
    assert!(h.state().ui.layer_zoom.unwrap() > z);
    command(&mut h, "view.fit");
    assert_eq!(h.state().ui.layer_zoom, None);
    assert!((shown(&h) - fit).abs() < 0.01);
    // With the Composition panel focused, the same commands zoom it instead.
    h.state_mut().show_panel(PanelKind::Composition);
    h.run_steps(2);
    command(&mut h, "view.zoomIn");
    assert!(h.state().ui.viewer.zoom.is_some());
    assert_eq!(h.state().ui.layer_zoom, None);
}

#[test]
fn wheel_zooms_around_the_pointer_and_hand_tool_pans() {
    let mut h = harness();
    let canvas = rect(&h, "layerPanel.canvas");
    // A point a quarter of the way into the layer stays under the pointer.
    let at = canvas.min + canvas.size() * 0.25;
    h.input_mut().events.push(Event::PointerMoved(at));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 120.0),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(20);
    let after = rect(&h, "layerPanel.canvas");
    assert!(after.width() > canvas.width(), "the wheel zooms in: {} → {}", canvas.width(), after.width());
    let still = after.min + after.size() * 0.25;
    assert!(still.distance(at) < 2.0, "the point under the pointer stays put: {at:?} vs {still:?}");
    // Hand tool drag pans; no paint or Roto stroke is made.
    h.state_mut().ui.tool = Tool::Hand;
    h.run_steps(2);
    let before = rect(&h, "layerPanel.canvas").min;
    let c = rect(&h, "layerPanel.area").center();
    drag_with(&mut h, egui::PointerButton::Primary, c, c + vec2(60.0, 30.0));
    let moved = rect(&h, "layerPanel.canvas").min - before;
    assert!((moved.x - 60.0).abs() < 1.0 && (moved.y - 30.0).abs() < 1.0, "{moved:?}");
    // Middle-drag pans with a Roto Brush tool, painting nothing.
    h.state_mut().ui.tool = Tool::RotoBrush;
    h.run_steps(2);
    let before = rect(&h, "layerPanel.canvas").min;
    drag_with(&mut h, egui::PointerButton::Middle, c, c - vec2(40.0, 20.0));
    let moved = rect(&h, "layerPanel.canvas").min - before;
    assert!((moved.x + 40.0).abs() < 1.0 && (moved.y + 20.0).abs() < 1.0, "{moved:?}");
    let layer = h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(box_id(&h))).unwrap().clone();
    assert!(layer.effects().is_none_or(|fx| fx.groups().count() == 0), "panning adds no Roto Brush effect");
}

#[test]
fn zoom_tool_click_zooms_in_the_layer_panel() {
    let mut h = harness();
    let fit = shown(&h);
    h.state_mut().ui.tool = Tool::Zoom;
    h.run_steps(2);
    let c = rect(&h, "layerPanel.canvas").center();
    click(&mut h, c);
    let z = h.state().ui.layer_zoom.expect("the Zoom tool sets the Layer panel's magnification");
    assert!((z - fit * 2.0).abs() < 0.01, "{fit} → {z}");
    assert_eq!(h.state().ui.viewer.zoom, None, "the Composition viewer keeps Fit");
}

#[test]
fn roto_brush_in_the_composition_panel_says_where_it_works() {
    let mut h = harness();
    h.state_mut().show_panel(PanelKind::Composition);
    h.state_mut().ui.layer_panel = None;
    h.state_mut().ui.tool = Tool::RotoBrush;
    h.run_steps(3);
    let comp = rect(&h, "viewer.comp");
    let c = comp.center();
    click(&mut h, c);
    assert!(h.state().ui.status.contains("Layer panel"), "a hint, not silence: {:?}", h.state().ui.status);
    // Double-clicking the layer opens it in the Layer panel.
    h.run_steps(30);
    h.input_mut().events.push(Event::PointerMoved(c));
    h.step();
    // Both clicks inside egui's double-click time; the second release (its own frame, as with a
    // real mouse) opens the layer after the press has focused the Composition panel.
    press(&mut h, c, egui::PointerButton::Primary, true);
    press(&mut h, c, egui::PointerButton::Primary, false);
    press(&mut h, c, egui::PointerButton::Primary, true);
    h.step();
    press(&mut h, c, egui::PointerButton::Primary, false);
    h.run_steps(3);
    assert_eq!(h.state().ui.layer_panel, Some(box_id(&h)));
    assert_eq!(h.state().ui.focused, PanelKind::Layer);
}
