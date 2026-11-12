//! The divider between the Timeline's layer columns and its time graph: dragging it (or setting
//! `split_w`) moves the graph's left edge; clearing `split_w` goes back to following the columns.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

/// A harness with one composition and one layer, so the Timeline has its columns and a graph.
fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": [1.0, 0.0, 0.0, 1.0]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

/// The divider's x position in the automation list, if it is on screen.
fn split_x(h: &Harness<'_, EffectcraftApp>) -> Option<f32> {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == "timeline.split").map(|e| e.rect.center().x)
}

#[test]
/// A set `split_w` moves the divider; clearing it follows the visible columns again.
fn split_width_moves_the_divider_and_none_resets_it() {
    let mut h = harness();
    let auto = split_x(&h).expect("the divider is registered");
    h.state_mut().ui.timeline.split_w = Some(1000.0);
    h.run_steps(3);
    let wide = split_x(&h).expect("the divider is registered");
    assert!(wide > auto, "a wider split moves the divider right ({wide} vs {auto})");
    h.state_mut().ui.timeline.split_w = Some(f32::NAN);
    h.run_steps(3);
    assert!(split_x(&h).is_some_and(|x| x.is_finite()), "a NaN width does not break the layout");
    h.state_mut().ui.timeline.split_w = None;
    h.run_steps(3);
    assert_eq!(split_x(&h), Some(auto), "None follows the columns again");
}
