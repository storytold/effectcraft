//! Headless Properties panel checks (egui_kittest): a solid's Roundness row rounds it into a
//! shape layer (one undo step, also when scrubbed), and a shape layer's Layer Contents section
//! lists its paths, Fill and Stroke with automation ids.

use effectcraft_engine::Session;
use effectcraft_engine::project::{Layer, LayerId, LayerSource};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::json;

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn ids(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect()
}

fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}")).clone();
    egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    h.step();
}

/// Click a hot number and type a value over it.
fn type_into(h: &mut Harness<'_, EffectcraftApp>, id: &str, text: &str) {
    let p = rect_of(h, id).center();
    click(h, p);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(text.into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

/// Drag one pixel per frame.
fn slow_drag(h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2, dx: f32) {
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=dx as i32 {
        h.event(egui::Event::PointerMoved(from + egui::vec2(k as f32, 0.0)));
        h.step();
    }
    let to = from + egui::vec2(dx, 0.0);
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

fn roundness(s: &Session, id: u64) -> Vec<f64> {
    let l = layer(s, id);
    effectcraft_engine::commands::round_corners::rect_paths(l.props.sub("contents").unwrap())
        .iter()
        .map(|r| r.get("roundness").unwrap().value.as_f64())
        .collect()
}

/// A 640×360 comp with a 200×100 red solid, selected, and the Properties panel shown.
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 4})).unwrap();
    let id = s.execute("layer.newSolid", json!({"name": "Red", "color": "#e02020", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [id]})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::Properties);
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, id)
}

#[test]
fn solid_roundness_row_makes_a_rounded_shape_layer() {
    let (mut h, id) = harness();
    assert!(ids(&h).contains(&"properties.solid.roundness".to_string()), "{:?}", ids(&h));
    type_into(&mut h, "properties.solid.roundness", "20");
    let l = layer(&h.state().session, id);
    assert!(matches!(l.source, LayerSource::Shape), "{:?}", l.source);
    assert_eq!(roundness(&h.state().session, id), vec![20.0]);
    // Now the Layer Contents section, in the Layer Transform row style.
    h.run_steps(3);
    let have = ids(&h);
    for want in [
        "properties.shape",
        "properties.shape.group",
        "properties.shape.reset",
        "properties.shape.size.value.0",
        "properties.shape.roundness.value",
        "properties.shape.roundness.stopwatch",
        "properties.shape.fillColor.value",
        "properties.shape.fillOpacity.value",
    ] {
        assert!(have.contains(&want.to_string()), "missing {want}: {have:?}");
    }
    assert!(!have.contains(&"properties.solid.roundness".to_string()));
    type_into(&mut h, "properties.shape.roundness.value", "8");
    assert_eq!(roundness(&h.state().session, id), vec![8.0]);
    // Edits of the field fold into one step (as a Layer Transform field's do): one undo
    // brings the solid back.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(matches!(layer(&h.state().session, id).source, LayerSource::Solid { .. }));
}

/// Scrubbing the solid's Roundness converts it and carries on on the shape layer's Roundness
/// row: one undo step for the whole gesture.
#[test]
fn a_roundness_scrub_is_one_undo_step() {
    let (mut h, id) = harness();
    let from = rect_of(&h, "properties.solid.roundness").center();
    slow_drag(&mut h, from, 30.0);
    let r = roundness(&h.state().session, id);
    assert!(r.len() == 1 && r[0] > 5.0, "{r:?}");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(matches!(layer(&h.state().session, id).source, LayerSource::Solid { .. }));
}

#[test]
fn shape_layers_list_their_groups_paths_fill_and_stroke() {
    let (mut h, _) = harness();
    let s = &mut h.state_mut().session;
    let l = s.execute("shape.newShape", json!({"kind": "star", "size": [100, 100], "strokeWidth": 3, "stroke": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("shape.newShape", json!({"layer": l, "kind": "ellipse", "size": [60, 40]})).unwrap();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    h.run_steps(4);
    let have = ids(&h);
    // Newest group first: the ellipse, then the star.
    for want in [
        "properties.shape.ellipseSize.value.0",
        "properties.shape.2.points.value",
        "properties.shape.2.outerRadius.value",
        "properties.shape.2.innerRoundness.value",
        "properties.shape.2.strokeColor.value",
        "properties.shape.2.strokeWidth.value",
    ] {
        assert!(have.contains(&want.to_string()), "missing {want}: {have:?}");
    }
    type_into(&mut h, "properties.shape.2.points.value", "7");
    let st = layer(&h.state().session, l);
    let points = st.props.prop("contents/#2/contents/star/points").unwrap().value.as_f64();
    assert_eq!(points, 7.0);
}
