//! Fast Previews ▸ Adaptive Resolution while dragging outside the viewer (After Effects): any
//! Timeline drag that changes the frame on screen (the time, or the comp's pixels) draws a draft
//! until release, and the first frame after the release is full resolution again. A plain click,
//! or a drag that changes nothing on screen, keeps Full.
use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// The Timeline of a 10 s comp at 30 fps with a solid (opacity keys at 0 s and 1 s, revealed)
/// and a 20 s precomp trimmed to 3–7 s (its source runs past both ends, so it can slip), in
/// Adaptive Resolution.
struct Fixture {
    h: Harness<'static, EffectcraftApp>,
    solid: u64,
    pre: u64,
    opacity: u64,
}

fn fixture(mode: &str) -> Fixture {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 320, "height": 180, "frameRate": 30, "duration": 20})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 10})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0.0, 0.0), (1.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": solid, "path": "transform/opacity", "time": time, "value": value})).unwrap();
    }
    let pre = s.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [pre], "in": 3.0, "out": 7.0})).unwrap();
    let comp = s.active_comp().unwrap();
    let opacity = comp.layers.iter().find(|l| l.id.0 == solid).unwrap().props.prop("transform/opacity").unwrap().uid;
    s.execute("layer.select", json!({"layers": [solid]})).unwrap();
    s.execute("view.fastPreviewMode", json!({"mode": mode})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    Fixture { h, solid, pre, opacity }
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// What a frame-changing drag must change: the project or the current time.
fn on_screen(h: &Harness<'_, EffectcraftApp>) -> (u64, effectcraft_time::Tick) {
    (h.state().session.revision, h.state().session.time())
}

/// Presses at `from` and moves to `to` in ten steps, leaving the button down.
fn hold(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for step in 1..=10 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (step as f32 / 10.0)));
        h.step();
    }
}

/// Releases at `at` and runs exactly one frame: that frame must already be sharp.
fn release(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
}

fn draft(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.state().viewer_scale(1.0, 1.0) < 1.0
}

/// Drags from `from` by `by`: a draft while held, Full on the first frame after release.
fn assert_draft_while_dragging(f: &mut Fixture, from: Pos2, by: egui::Vec2) {
    let before = on_screen(&f.h);
    hold(&mut f.h, from, from + by);
    assert_ne!(on_screen(&f.h), before, "the drag changed nothing on screen");
    assert!(f.h.state().ui.viewer.property_interacting, "not interacting while dragging");
    assert!(draft(&f.h), "full resolution while dragging");
    release(&mut f.h, from + by);
    assert!(!f.h.state().ui.viewer.property_interacting, "still interacting after release");
    assert!(!draft(&f.h), "the first frame after release is a draft");
}

#[test]
fn playhead_drag_draws_a_draft() {
    let mut f = fixture("adaptive");
    let ruler = rect(&f.h, "timeline.ruler");
    assert_draft_while_dragging(&mut f, pos2(ruler.min.x + 40.0, ruler.max.y - 3.0), vec2(200.0, 0.0));
}

#[test]
fn timecode_drag_draws_a_draft() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, "timeline.timecode").center();
    assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
}

#[test]
fn layer_bar_drag_draws_a_draft() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, &format!("timeline.layer.{}.bar", f.solid)).center();
    assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
}

#[test]
fn in_point_trim_draws_a_draft() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, &format!("timeline.layer.{}.bar.in", f.pre)).center();
    assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
}

#[test]
fn source_slip_draws_a_draft() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, &format!("timeline.layer.{}.sourcel", f.pre)).center();
    assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
}

#[test]
fn key_drag_draws_a_draft() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, &format!("timeline.key.{}.0", f.opacity)).center();
    assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
}

#[test]
fn in_out_and_duration_cell_drags_draw_a_draft() {
    for kind in ["in", "out", "duration"] {
        let mut f = fixture("adaptive");
        effectcraft_ui_egui::panels::timeline::set_column(&mut f.h.state_mut().ui.timeline, kind, true).unwrap();
        f.h.run_steps(3);
        let from = rect(&f.h, &format!("timeline.layer.{}.{kind}", f.pre)).center();
        assert_draft_while_dragging(&mut f, from, vec2(60.0, 0.0));
    }
}

#[test]
fn properties_scrub_draws_a_draft() {
    let mut f = fixture("adaptive");
    let ctx = f.h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(f.h.state_mut(), &ctx, "window.panel", json!({"panel": "properties"})).unwrap();
    f.h.run_steps(3);
    let from = rect(&f.h, &format!("properties.prop.{}.value", f.opacity)).center();
    assert_draft_while_dragging(&mut f, from, vec2(40.0, 0.0));
}

#[test]
fn work_area_drag_keeps_full_resolution() {
    let mut f = fixture("adaptive");
    let from = rect(&f.h, "timeline.workArea.end").center();
    let before = f.h.state().session.active_comp().unwrap().work_area;
    hold(&mut f.h, from, from + vec2(-80.0, 0.0));
    assert_ne!(f.h.state().session.active_comp().unwrap().work_area, before, "the work area moved");
    assert!(!draft(&f.h), "a work-area drag changes no pixels");
    release(&mut f.h, from);
}

#[test]
fn timeline_click_keeps_full_resolution() {
    let mut f = fixture("adaptive");
    let point = rect(&f.h, "timeline.ruler").center();
    f.h.input_mut().events.push(Event::PointerMoved(point));
    f.h.step();
    f.h.input_mut().events.push(Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    f.h.run_steps(3);
    assert!(!draft(&f.h));
    release(&mut f.h, point);
}

#[test]
fn fast_previews_off_keeps_full_resolution() {
    let mut f = fixture("off");
    let ruler = rect(&f.h, "timeline.ruler");
    let from = pos2(ruler.min.x + 40.0, ruler.max.y - 3.0);
    hold(&mut f.h, from, from + vec2(200.0, 0.0));
    assert!(!draft(&f.h));
    release(&mut f.h, from);
}
