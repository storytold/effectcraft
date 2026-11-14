//! Graph Editor value-axis navigation: Ctrl/Cmd+wheel (and a pinch) zooms the value axis
//! instead of the time ruler, a plain wheel pans it without scrolling the layer outline, and
//! middle-drag / Space-hand pan it too. These are the AE-parity controls the Graph Editor was
//! missing: before the fix, Ctrl/Cmd+wheel was delivered by egui as `zoom_delta()` (so the old
//! `smooth_scroll_delta` check never fired) and the other gestures moved only the layer rows.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, MouseWheelUnit, Pos2, Vec2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A 20 s comp with 41 rows (so the layer outline can scroll) and a keyframed Plate in the
/// middle, with the Graph Editor open. Auto-zoom is left on so the tests can see it switch off.
fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Nav", "width": 320, "height": 180, "frameRate": 30, "duration": 20})).unwrap();
    for i in 0..20 {
        s.execute("layer.newSolid", json!({"name": format!("Under {i}"), "color": "#406080"})).unwrap();
    }
    let layer = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(2.0, 300.0), (6.0, 720.0)] {
        s.execute("prop.addKey", json!({"layer": layer, "path": "transform/rotation", "time": t, "value": v})).unwrap();
    }
    for i in 0..20 {
        s.execute("layer.newSolid", json!({"name": format!("Over {i}"), "color": "#608040"})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [layer]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.pps = Some(300.0);
    h.state_mut().ui.timeline.start = 0.0;
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.graphEditor", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().ui.timeline.graph_editor);
    h
}

fn plot(h: &Harness<'_, EffectcraftApp>) -> egui::Rect {
    let e = h.state().auto.find("timeline.graph").expect("graph plot").clone();
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn range(h: &Harness<'_, EffectcraftApp>) -> (f64, f64) {
    h.state().ui.timeline.graph_range.expect("graph range")
}

/// Put the layer outline at `y` and return it; the fixture must have room to scroll, otherwise
/// the "outline moved" checks could pass by clamping at zero.
fn outline_scroll(h: &mut Harness<'_, EffectcraftApp>, y: f32) -> f32 {
    h.state_mut().ui.timeline.scroll_y = y;
    h.run_steps(1);
    let after = h.state().ui.timeline.scroll_y;
    assert!((after - y).abs() < 1e-3, "the fixture has no outline scroll room: {y} -> {after}");
    after
}

fn wheel(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, delta: Vec2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(at));
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::MouseWheel { unit: MouseWheelUnit::Point, delta, modifiers, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, button: egui::PointerButton, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn space(h: &mut Harness<'_, EffectcraftApp>, pressed: bool) {
    h.event(Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

/// Ctrl/Cmd+wheel over the plot zooms the value axis (around the pointer) and leaves the time
/// ruler alone: egui hands Ctrl/Cmd+wheel over as `zoom_delta()`, which the Graph Editor now
/// consumes instead of letting the Timeline zoom time with it.
#[test]
fn ctrl_wheel_zooms_the_value_axis_not_time() {
    let mut h = harness();
    h.state_mut().ui.timeline.pps = Some(300.0);
    h.state_mut().ui.timeline.start = 1.0;
    h.run_steps(2);
    let scroll = outline_scroll(&mut h, 1.0);
    assert!(h.state().ui.timeline.graph_auto_zoom, "the fixture starts with Auto-zoom on");
    let before = range(&h);
    let time = (h.state().ui.timeline.pps, h.state().ui.timeline.start);
    let at = plot(&h).center();
    wheel(&mut h, at, vec2(0.0, 200.0), Modifiers::COMMAND);
    let after = range(&h);
    assert!(!h.state().ui.timeline.graph_auto_zoom, "manual navigation turns Auto-zoom off");
    assert!(after.1 - after.0 < before.1 - before.0, "Ctrl/Cmd+wheel zooms the value span in: {before:?} -> {after:?}");
    assert_eq!((h.state().ui.timeline.pps, h.state().ui.timeline.start), time, "the time zoom and start are untouched");
    assert_eq!(h.state().ui.timeline.scroll_y, scroll, "the layer outline did not scroll");
}

/// A plain wheel over the plot pans the value axis; it no longer scrolls the layer outline.
#[test]
fn plain_wheel_pans_the_value_axis_without_scrolling_the_outline() {
    let mut h = harness();
    let scroll = outline_scroll(&mut h, 1.0);
    assert!(h.state().ui.timeline.graph_auto_zoom, "the fixture starts with Auto-zoom on");
    let before = range(&h);
    let at = plot(&h).center();
    wheel(&mut h, at, vec2(0.0, 120.0), Modifiers::NONE);
    let after = range(&h);
    assert!(!h.state().ui.timeline.graph_auto_zoom, "manual navigation turns Auto-zoom off");
    assert!((after.0 - before.0).abs() > 1.0, "the value range pans: {before:?} -> {after:?}");
    assert!(((after.1 - after.0) - (before.1 - before.0)).abs() < 1e-6, "the value span is unchanged");
    assert_eq!(h.state().ui.timeline.scroll_y, scroll, "the layer outline did not scroll");
}

/// A middle-drag that starts on the plot pans the value axis, not the layer outline.
#[test]
fn middle_drag_pans_the_value_axis() {
    let mut h = harness();
    let scroll = outline_scroll(&mut h, 1.0);
    assert!(h.state().ui.timeline.graph_auto_zoom, "the fixture starts with Auto-zoom on");
    let before = range(&h);
    let at = plot(&h).center();
    drag(&mut h, egui::PointerButton::Middle, at, at + vec2(0.0, 60.0));
    let after = range(&h);
    assert!(!h.state().ui.timeline.graph_auto_zoom, "manual navigation turns Auto-zoom off");
    assert!((after.0 - before.0).abs() > 1.0, "the value range pans: {before:?} -> {after:?}");
    assert_eq!(h.state().ui.timeline.scroll_y, scroll, "the layer outline did not scroll");
}

/// The Spacebar hand pans the value axis over the plot, as it pans the viewer elsewhere.
#[test]
fn space_hand_pans_the_value_axis() {
    let mut h = harness();
    let scroll = outline_scroll(&mut h, 1.0);
    assert!(h.state().ui.timeline.graph_auto_zoom, "the fixture starts with Auto-zoom on");
    let before = range(&h);
    let at = plot(&h).center();
    space(&mut h, true);
    drag(&mut h, egui::PointerButton::Primary, at, at + vec2(0.0, 60.0));
    space(&mut h, false);
    let after = range(&h);
    assert!(!h.state().ui.timeline.graph_auto_zoom, "manual navigation turns Auto-zoom off");
    assert!((after.0 - before.0).abs() > 1.0, "the value range pans: {before:?} -> {after:?}");
    assert_eq!(h.state().ui.timeline.scroll_y, scroll, "the layer outline did not scroll");
}

/// Alt+wheel over the plot still zooms time (the gesture it kept from #428), and horizontal /
/// Shift+wheel still pans time; neither moves the graph value range.
#[test]
fn alt_wheel_zooms_time_and_horizontal_wheel_pans_time_over_the_plot() {
    let mut h = harness();
    h.state_mut().ui.timeline.pps = Some(300.0);
    h.state_mut().ui.timeline.start = 1.0;
    // A manual range so the check is about this gesture, not an Auto-zoom refit after the time
    // span changes.
    h.state_mut().ui.timeline.graph_auto_zoom = false;
    h.state_mut().ui.timeline.graph_range = Some((0.0, 800.0));
    h.run_steps(2);
    let at = plot(&h).center();
    let before = (h.state().ui.timeline.pps.expect("pps"), range(&h));
    wheel(&mut h, at, vec2(0.0, 200.0), Modifiers::ALT);
    let pps = h.state().ui.timeline.pps.expect("time still zooms");
    assert!(pps > before.0, "Alt+wheel zooms time in: {} -> {pps}", before.0);
    assert_eq!(range(&h), before.1, "Alt+wheel leaves the value range alone");
    assert!(!h.state().ui.timeline.graph_auto_zoom, "Alt+wheel is not a value gesture");

    let before = (h.state().ui.timeline.start, range(&h));
    wheel(&mut h, at, vec2(120.0, 0.0), Modifiers::NONE);
    assert!(h.state().ui.timeline.start < before.0, "horizontal wheel pans time: {} -> {}", before.0, h.state().ui.timeline.start);
    assert_eq!(range(&h), before.1, "horizontal wheel leaves the value range alone");
    assert!(!h.state().ui.timeline.graph_auto_zoom);
}

/// Repeated zooming at a large value cannot collapse the span below one f64 step: that used to
/// make the value-grid loop stop advancing and freeze the app.
#[test]
fn repeated_zoom_at_large_values_keeps_a_finite_span() {
    let mut h = harness();
    h.state_mut().ui.timeline.graph_auto_zoom = false;
    h.state_mut().ui.timeline.graph_range = Some((19_999_999.0, 20_000_001.0));
    h.run_steps(2);
    let at = plot(&h).center();
    for _ in 0..40 {
        wheel(&mut h, at, vec2(0.0, 240.0), Modifiers::COMMAND);
    }
    let (lo, hi) = range(&h);
    assert!(lo.is_finite() && hi.is_finite(), "the value range stays finite: {lo}..{hi}");
    assert!(hi > lo, "the value range stays ordered: {lo}..{hi}");
    assert!(hi - lo >= 0.019, "the span is bounded at this magnitude: {lo}..{hi}");
}
