//! Trimming layers in the Timeline as in After Effects: the pointer over either end of a layer
//! bar (a few points inside it or just outside) shows the trim cursor, and dragging that end
//! moves the layer's In or Out point.
use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{CursorIcon, Event, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A 10 s comp at 30 fps with a solid trimmed to 2–6 s and a 20 s precomp trimmed to 3–7 s (its
/// source runs past both ends, so the slip zones sit right beside its bar's ends).
fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 320, "height": 180, "frameRate": 30, "duration": 20})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 10})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [solid], "in": 2.0, "out": 6.0})).unwrap();
    let pre = s.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [pre], "in": 3.0, "out": 7.0})).unwrap();
    s.execute("layer.select", json!({"layers": []})).unwrap();
    let app = EffectcraftApp::new(s);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, solid, pre)
}

fn bar(h: &Harness<'_, EffectcraftApp>, layer: u64) -> egui::Rect {
    let e = h.state().auto.find(&format!("timeline.layer.{layer}.bar")).expect("layer bar").clone();
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// The cursor shown with the pointer resting at `p`.
fn cursor_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) -> CursorIcon {
    h.event(Event::PointerMoved(p));
    h.run_steps(3);
    h.output().platform_output.cursor_icon
}

/// (in, out, start) of `layer` in seconds.
fn timing(h: &Harness<'_, EffectcraftApp>, layer: u64) -> (f64, f64, f64) {
    let comp = h.state().session.active_comp().unwrap();
    let l = comp.layer(LayerId(layer)).unwrap();
    (l.in_point.seconds(), l.out_point.seconds(), l.start_time.seconds())
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    drag_with(h, from, to, Modifiers::NONE);
}

fn drag_with(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(from));
    h.event(Event::ModifiersChanged(modifiers));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

/// Shift-dragging a bar end or the bar snaps it to another layer's In / Out point a few pixels
/// away; without Shift it stops on the frame the pointer reached (#1828).
#[test]
fn shift_dragging_a_bar_snaps_to_other_layers() {
    for shift in [false, true] {
        let (mut h, solid, pre) = harness();
        let mods = if shift { Modifiers::SHIFT } else { Modifiers::NONE };
        let (b, p) = (bar(&h, solid), bar(&h, pre));
        let y = b.center().y;
        // The solid's Out end (6 s), grabbed 2 px inside, to 6 px short of the precomp's Out (7 s).
        drag_with(&mut h, pos2(b.max.x - 2.0, y), pos2(p.max.x - 8.0, y), mods);
        let (i, o, _) = timing(&h, solid);
        assert!((i - 2.0).abs() < 1e-6, "shift {shift}: in stays at 2 s, got {i}");
        assert_eq!((o - 7.0).abs() < 1e-6, shift, "shift {shift}: out {o} snaps to 7 s only with Shift");
        // The whole bar, so its In (2 s) lands 5 px short of the precomp's In (3 s).
        let b = bar(&h, solid);
        drag_with(&mut h, pos2(b.center().x, y), pos2(b.center().x + p.min.x - b.min.x - 5.0, y), mods);
        let (i, _, _) = timing(&h, solid);
        assert_eq!((i - 3.0).abs() < 1e-6, shift, "shift {shift}: in {i} snaps to 3 s only with Shift");
    }
}

/// Both ends of every bar show the trim cursor, just inside and just outside the end; the middle
/// doesn't. The same holds for a selected layer, a twirled-open layer and a short bar.
fn assert_trim_cursor_at_ends(h: &mut Harness<'_, EffectcraftApp>, layer: u64, case: &str) {
    let b = bar(h, layer);
    let y = b.center().y;
    for (what, x) in [("in, outside", b.min.x - 3.0), ("in, inside", b.min.x + 2.0), ("out, inside", b.max.x - 2.0), ("out, outside", b.max.x + 3.0)] {
        assert_eq!(cursor_at(h, pos2(x, y)), CursorIcon::ResizeHorizontal, "{case}: layer {layer} {what} (bar {b:?})");
    }
    if b.width() > 40.0 {
        assert_ne!(cursor_at(h, b.center()), CursorIcon::ResizeHorizontal, "{case}: the middle of the bar moves, it doesn't trim");
    }
}

#[test]
fn bar_ends_show_the_trim_cursor() {
    let (mut h, solid, pre) = harness();
    assert_trim_cursor_at_ends(&mut h, solid, "unselected solid");
    assert_trim_cursor_at_ends(&mut h, pre, "precomp with slip zones beside its ends");
    h.state_mut().session.execute("layer.select", json!({"layers": [solid]})).unwrap();
    h.run_steps(2);
    assert_trim_cursor_at_ends(&mut h, solid, "selected");
    h.state_mut().ui.timeline.open_layers.insert(solid);
    h.run_steps(2);
    assert_trim_cursor_at_ends(&mut h, solid, "twirled open");
    assert_trim_cursor_at_ends(&mut h, pre, "below a twirled-open layer");
    // Three frames long: still trims from both ends.
    h.state_mut().session.execute("layer.timing", json!({"layers": [solid], "in": 2.0, "out": 2.1})).unwrap();
    h.run_steps(2);
    assert_trim_cursor_at_ends(&mut h, solid, "short bar");
}

/// The trim zones show the horizontal resize cursor; the bar body doesn't.
#[test]
fn trim_zones_show_the_resize_cursor() {
    let (mut h, solid, _) = harness();
    let b = bar(&h, solid);
    assert_eq!(cursor_at(&mut h, pos2(b.max.x, b.center().y)), CursorIcon::ResizeHorizontal);
    assert_ne!(cursor_at(&mut h, b.center()), CursorIcon::ResizeHorizontal, "trim cursor leaked onto the bar body");
}

#[test]
fn dragging_bar_ends_trims_in_and_out() {
    let (mut h, solid, pre) = harness();
    let b = bar(&h, solid);
    let pps = b.width() as f64 / 4.0;
    let y = b.center().y;
    // Out end, grabbed just inside, one second to the left.
    drag(&mut h, pos2(b.max.x - 2.0, y), pos2(b.max.x - 2.0 - pps as f32, y));
    let (i, o, s) = timing(&h, solid);
    assert!((i - 2.0).abs() < 1e-6 && (o - 5.0).abs() < 1.5 / 30.0 && s.abs() < 1e-6, "out trimmed to ~5 s: in {i} out {o} start {s}");
    // In end, grabbed just inside, one second to the right.
    let b = bar(&h, solid);
    drag(&mut h, pos2(b.min.x + 2.0, y), pos2(b.min.x + 2.0 + pps as f32, y));
    let (i, o2, s) = timing(&h, solid);
    assert!((i - 3.0).abs() < 1.5 / 30.0 && (o2 - o).abs() < 1e-6 && s.abs() < 1e-6, "in trimmed to ~3 s: in {i} out {o2} start {s}");
    // The precomp's Out end grabbed just outside the bar (over its slip zone) extends it.
    let b = bar(&h, pre);
    let y = b.center().y;
    drag(&mut h, pos2(b.max.x + 3.0, y), pos2(b.max.x + 3.0 + pps as f32, y));
    let (i, o, s) = timing(&h, pre);
    assert!((i - 3.0).abs() < 1e-6 && (o - 8.0).abs() < 1.5 / 30.0 && s.abs() < 1e-6, "out extended to ~8 s, not slipped: in {i} out {o} start {s}");
}

/// A Shift-drag on an unselected bar adds it to the selection and moves the whole selection
/// together, so the other selected layers keep their offset (#1828).
#[test]
fn shift_dragging_an_unselected_bar_moves_the_selection() {
    let (mut h, solid, pre) = harness();
    h.state_mut().session.execute("layer.select", json!({"layers": [pre]})).unwrap();
    h.run_steps(2);
    let b = bar(&h, solid);
    drag_with(&mut h, b.center(), b.center() + vec2(b.width() / 4.0 * 2.5, 0.0), Modifiers::SHIFT);
    let ((si, ..), (pi, ..)) = (timing(&h, solid), timing(&h, pre));
    assert!(si > 4.0 && (pi - si - 1.0).abs() < 1e-6, "both moved by the same amount: solid in {si}, precomp in {pi}");
}

/// With Frames and Frame Count Start at 1, In moves to 00061 but the Duration column still counts
/// the layer's 120 frames (#1807).
#[test]
fn duration_column_counts_frames_from_any_frame_count() {
    let (mut h, solid, _) = harness();
    effectcraft_ui_egui::panels::timeline::set_column(&mut h.state_mut().ui.timeline, "in", true).unwrap();
    effectcraft_ui_egui::panels::timeline::set_column(&mut h.state_mut().ui.timeline, "duration", true).unwrap();
    h.state_mut().session.execute("file.projectSettings", json!({"timeDisplay": "frames", "frameStart": 1})).unwrap();
    h.run_steps(3);
    let cell = |h: &Harness<'_, EffectcraftApp>, kind: &str| h.state().auto.find(&format!("timeline.layer.{solid}.{kind}")).expect("cell").label.clone();
    assert_eq!(cell(&h, "in"), "00061");
    assert_eq!(cell(&h, "duration"), "00120");
}
