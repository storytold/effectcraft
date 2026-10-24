use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, LayerId, u64) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Keys", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    session.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = session.active_comp().unwrap().layers[0].id;
    for (time, value) in [(0.0, 0.0), (1.0, 100.0)] {
        session.execute("prop.addKey", json!({"layer": id.0, "path": "transform/opacity", "time": time, "value": value})).unwrap();
    }
    let uid = session.active_comp().unwrap().layer(id).unwrap().props.prop("transform/opacity").unwrap().uid;
    session.execute("layer.select", json!({"layers": [id.0]})).unwrap();
    session.execute("view.fastPreviewMode", json!({"mode": "adaptive"})).unwrap();
    let mut harness = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    harness.run_steps(3);
    let context = harness.ctx.clone();
    effectcraft_ui_egui::menus::invoke(harness.state_mut(), &context, "timeline.reveal.opacity", json!({})).unwrap();
    harness.run_steps(4);
    (harness, id, uid)
}

fn rect(harness: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let entry = harness.state().auto.find(id).expect("timeline element");
    egui::Rect::from_min_size(pos2(entry.rect[0], entry.rect[1]), vec2(entry.rect[2], entry.rect[3]))
}

fn keys(harness: &Harness<'_, EffectcraftApp>, uid: u64) -> Vec<Pos2> {
    let mut positions: Vec<Pos2> = harness
        .state()
        .auto
        .query(&format!("timeline.key.{uid}."))
        .iter()
        .map(|entry| pos2(entry.rect[0] + entry.rect[2] / 2.0, entry.rect[1] + entry.rect[3] / 2.0))
        .collect();
    positions.sort_by(|left, right| left.x.total_cmp(&right.x));
    positions
}

fn hold(harness: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    harness.input_mut().events.push(Event::PointerMoved(from));
    harness.step();
    harness.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    harness.step();
    for step in 1..=10 {
        harness.input_mut().events.push(Event::PointerMoved(from + (to - from) * (step as f32 / 10.0)));
        harness.step();
    }
}

fn release(harness: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    harness.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    harness.run_steps(2);
}

fn assert_adaptive_during_drag(harness: &Harness<'_, EffectcraftApp>) {
    assert!(harness.state().ui.viewer.property_interacting);
    assert!(harness.state().viewer_scale(1.0, 1.0) < 1.0);
}

fn assert_full_resolution_after_release(harness: &Harness<'_, EffectcraftApp>) {
    assert!(!harness.state().ui.viewer.property_interacting);
    assert_eq!(harness.state().viewer_scale(1.0, 1.0), 1.0);
}

#[test]
fn playhead_drag_uses_adaptive_resolution() {
    let (mut harness, _, _) = harness();
    let ruler = rect(&harness, "timeline.ruler");
    let from = pos2(ruler.min.x + 40.0, ruler.max.y - 3.0);
    let to = from + vec2(200.0, 0.0);
    hold(&mut harness, from, to);
    assert_adaptive_during_drag(&harness);
    release(&mut harness, to);
    assert_full_resolution_after_release(&harness);
}

#[test]
fn layer_bar_drag_uses_adaptive_resolution() {
    let (mut harness, layer, _) = harness();
    let from = rect(&harness, &format!("timeline.layer.{}.bar", layer.0)).center();
    let to = from + vec2(60.0, 0.0);
    hold(&mut harness, from, to);
    assert_adaptive_during_drag(&harness);
    release(&mut harness, to);
    assert_full_resolution_after_release(&harness);
}

#[test]
fn key_drag_uses_adaptive_resolution() {
    let (mut harness, _, uid) = harness();
    let from = keys(&harness, uid)[0];
    let to = from + vec2(60.0, 0.0);
    hold(&mut harness, from, to);
    assert_adaptive_during_drag(&harness);
    release(&mut harness, to);
    assert_full_resolution_after_release(&harness);
}

#[test]
fn timeline_click_keeps_full_resolution() {
    let (mut harness, _, _) = harness();
    let point = rect(&harness, "timeline.ruler").center();
    harness.input_mut().events.push(Event::PointerMoved(point));
    harness.step();
    harness.input_mut().events.push(Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    harness.step();
    assert!(!harness.state().ui.viewer.property_interacting);
    assert_eq!(harness.state().viewer_scale(1.0, 1.0), 1.0);
    release(&mut harness, point);
}
