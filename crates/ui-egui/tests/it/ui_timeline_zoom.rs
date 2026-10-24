//! The Timeline's time ruler with real input: Alt+wheel zooms out until the whole comp shows
//! (#158); the Time Navigator's ends drag to zoom and `;` toggles frame level / the whole comp
//! (#159); markers dragged out of the composition marker bin (#275).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, MouseWheelUnit, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// A 10-minute comp with one solid, zoomed in on its start.
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Long", "width": 320, "height": 180, "frameRate": 30, "duration": 600})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.pps = Some(200.0);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(3);
    (h, layer)
}

/// A point over the layer's bar in the time graph.
fn over_bar(h: &Harness<'_, EffectcraftApp>, layer: u64) -> Pos2 {
    let e = h.state().auto.find(&format!("timeline.layer.{layer}.bar")).expect("layer bar").clone();
    pos2(e.rect[0] + 40.0, e.rect[1] + e.rect[3] / 2.0)
}

fn alt_wheel(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, dy: f32) {
    h.event(Event::PointerMoved(at));
    h.event(Event::ModifiersChanged(Modifiers::ALT));
    h.event(Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, dy), modifiers: Modifiers::ALT, phase: egui::TouchPhase::Move });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

/// #158: on a long comp, Alt+wheel zooms out all the way to the whole comp (and no further).
#[test]
fn alt_wheel_zooms_out_until_the_whole_comp_shows() {
    let (mut h, layer) = harness();
    for _ in 0..80 {
        let at = over_bar(&h, layer);
        alt_wheel(&mut h, at, -120.0);
        if h.state().ui.timeline.pps.is_none() {
            break;
        }
    }
    let tl = &h.state().ui.timeline;
    assert_eq!((tl.pps, tl.start), (None, 0.0), "fits the whole comp: {:?}", tl.pps);
    // The bar spans (nearly) the whole time graph.
    let bar = h.state().auto.find(&format!("timeline.layer.{layer}.bar")).unwrap().rect;
    let ruler = h.state().auto.find("timeline.ruler").unwrap().rect;
    assert!(bar[2] > ruler[2] * 0.95, "bar {bar:?} ruler {ruler:?}");
    // Zooming in again works from there.
    let at = over_bar(&h, layer);
    alt_wheel(&mut h, at, 240.0);
    assert!(h.state().ui.timeline.pps.is_some());
}

/// #428: Ctrl/Cmd+wheel and a trackpad pinch zoom time over the layers, as Alt+wheel does (egui
/// hands them over as a zoom factor, which the Timeline ignored).
#[test]
fn ctrl_wheel_and_pinch_zoom_time() {
    let (mut h, layer) = harness();
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.run_steps(2);
    let pps = |h: &Harness<'_, EffectcraftApp>| h.state().ui.timeline.pps.unwrap_or(0.0);
    let start = pps(&h);
    assert!(start > 0.0);
    let at = over_bar(&h, layer);
    h.event(Event::PointerMoved(at));
    h.event(Event::Zoom(0.5));
    h.step();
    let pinched = pps(&h);
    assert!(pinched > 0.0 && pinched < start * 0.75, "a pinch in zooms out: {start} -> {pinched}");
    let at = over_bar(&h, layer);
    h.event(Event::PointerMoved(at));
    h.event(Event::ModifiersChanged(Modifiers::COMMAND));
    h.event(Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, 240.0), modifiers: Modifiers::COMMAND, phase: egui::TouchPhase::Move });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    assert!(pps(&h) > pinched, "Ctrl/Cmd+wheel up zooms in: {pinched} -> {}", pps(&h));
    // Over the layers, so the rows did not scroll.
    assert_eq!(h.state().ui.timeline.scroll_y, 0.0);
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    drag_with(h, from, to, Modifiers::NONE);
}

/// [`drag`] with `modifiers` held.
fn drag_with(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for k in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

/// The visible span (seconds) from the ruler's automation label ("start,pps") and width.
fn visible(h: &Harness<'_, EffectcraftApp>) -> (f64, f64) {
    let e = h.state().auto.find("timeline.ruler").unwrap().clone();
    let (start, pps) = e.label.split_once(',').map(|(a, b)| (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap())).unwrap();
    // (The time graph ends 18 px before the ruler, at the marker bin; it maps time from 6 px in.)
    (start, start + (e.rect[2] - 24.0) as f64 / pps)
}

/// #159: dragging the navigator's end handles changes that side of the visible span; pulling
/// them out to the track's ends shows the whole comp.
#[test]
fn navigator_ends_drag_to_zoom() {
    let (mut h, _) = harness();
    h.state_mut().ui.timeline.pps = None;
    h.run_steps(3);
    let track = rect(&h, "timeline.navigator");
    // The end handle pulled to the middle: the span ends near half the comp, starts at 0.
    let end = rect(&h, "timeline.navigator.end").center();
    drag(&mut h, end, pos2(track.center().x, end.y));
    let (a, b) = visible(&h);
    assert!(a.abs() < 1e-6 && (b - 300.0).abs() < 15.0, "{a}..{b}");
    // The start handle pulled to a quarter: the span starts there, its end stays.
    let start = rect(&h, "timeline.navigator.start").center();
    drag(&mut h, start, pos2(track.min.x + track.width() * 0.25, start.y));
    let (a2, b2) = visible(&h);
    assert!((a2 - 150.0).abs() < 15.0 && (b2 - b).abs() < 2.0, "{a2}..{b2} (end was {b})");
    // Both ends back out to the track's ends: the whole comp.
    let start = rect(&h, "timeline.navigator.start").center();
    drag(&mut h, start, pos2(track.min.x - 20.0, start.y));
    let end = rect(&h, "timeline.navigator.end").center();
    drag(&mut h, end, pos2(track.max.x + 20.0, end.y));
    assert_eq!(h.state().ui.timeline.pps, None, "the whole comp");
}

/// #159: `;` zooms in to frame level around the current time, and back out to the whole comp.
#[test]
fn semicolon_toggles_frame_level_and_the_whole_comp() {
    let (mut h, _) = harness();
    h.state_mut().ui.timeline.pps = None;
    h.state_mut().session.execute("time.set", json!({"time": 120.0})).unwrap();
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFrameToggle", json!({})).unwrap();
    h.run_steps(2);
    let (a, b) = visible(&h);
    assert!(a < 120.0 && 120.0 < b && b - a < 1.0, "frame level around the CTI: {a}..{b}");
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFrameToggle", json!({})).unwrap();
    assert_eq!(h.state().ui.timeline.pps, None);
}

/// `Shift+;` zooms out to the whole comp, and again back in to the span it left.
#[test]
fn shift_semicolon_toggles_the_whole_comp_and_the_span_before() {
    let (mut h, _) = harness();
    h.state_mut().session.execute("time.set", json!({"time": 120.0})).unwrap();
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFrameToggle", json!({})).unwrap();
    h.run_steps(2);
    let before = (h.state().ui.timeline.start, h.state().ui.timeline.pps);
    assert!(before.1.is_some());
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFit", json!({})).unwrap();
    assert_eq!(h.state().ui.timeline.pps, None);
    h.run_steps(2);
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFit", json!({})).unwrap();
    assert_eq!((h.state().ui.timeline.start, h.state().ui.timeline.pps), before);
    // With nothing to go back to, the whole comp stays.
    h.state_mut().ui.timeline.pps = None;
    h.state_mut().ui.timeline.before_fit = None;
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.zoomFit", json!({})).unwrap();
    assert_eq!(h.state().ui.timeline.pps, None);
}

fn space(h: &mut Harness<'_, EffectcraftApp>, pressed: bool) {
    h.event(Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

/// #227: with Spacebar held, a drag over the time graph scrolls it in time (the Hand tool)
/// instead of moving the layer bar under the pointer, and the release doesn't preview.
#[test]
fn spacebar_drag_scrolls_the_time_graph() {
    let (mut h, layer) = harness();
    // (Opening the comp showed all of it.)
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.state_mut().ui.timeline.start = 10.0;
    h.run_steps(2);
    let in_point = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers[0].in_point.seconds();
    let from = pos2(rect(&h, "timeline.ruler").center().x, over_bar(&h, layer).y);
    // 400 px to the left at 200 px/s: 2 s later.
    space(&mut h, true);
    drag(&mut h, from, from - vec2(400.0, 0.0));
    space(&mut h, false);
    let (a, _) = visible(&h);
    assert!((a - 12.0).abs() < 0.01, "starts at {a}");
    assert_eq!(in_point(&h), 0.0, "the bar didn't move");
    assert!(!h.state().playback.playing, "a Spacebar drag doesn't preview");
    // To the right, at most back to the comp's start.
    space(&mut h, true);
    for _ in 0..6 {
        drag(&mut h, from, from + vec2(500.0, 0.0));
    }
    space(&mut h, false);
    assert_eq!(visible(&h).0, 0.0);
    assert_eq!(in_point(&h), 0.0);
    // Without Spacebar the same drag moves the bar.
    drag(&mut h, from, from - vec2(400.0, 0.0));
    assert!(in_point(&h) < -1.0, "{}", in_point(&h));
}

/// #290: right-clicking the work area bar offers its commands; Trim Comp to Work Area trims.
#[test]
fn work_area_context_menu_trims_the_comp() {
    let (mut h, _) = harness();
    h.state_mut().ui.timeline.pps = None;
    h.state_mut().session.execute("comp.workArea", json!({"start": 60.0, "end": 180.0})).unwrap();
    h.run_steps(3);
    let at = rect(&h, "timeline.workArea.bar").center();
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    for label in ["Lift Work Area", "Extract Work Area"] {
        assert!(h.query_by_label_contains(label).is_some(), "{label}");
    }
    let trim = h.query_by_label_contains("Trim Comp to Work Area").expect("Trim Comp to Work Area").rect().center();
    h.event(Event::PointerMoved(trim));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: trim, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert_eq!(h.state().session.active_comp().unwrap().duration.seconds(), 120.0);
}

/// #252: Shift-dragging a work area end, or the whole work area bar, snaps to the current-time
/// indicator when it comes within reach; without Shift it lands where the pointer is.
#[test]
fn shift_dragging_the_work_area_snaps_to_the_current_time() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Short", "width": 320, "height": 180, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.pps = Some(100.0);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    let reset = |h: &mut Harness<'_, EffectcraftApp>| {
        h.state_mut().session.execute("comp.workArea", json!({"start": 1.0, "end": 4.0})).unwrap();
        h.state_mut().session.execute("time.set", json!({"time": 6.0})).unwrap();
        h.run_steps(3);
    };
    let work_area = |h: &Harness<'_, EffectcraftApp>| {
        let wa = h.state().session.active_comp().unwrap().work_area;
        (wa.0.seconds(), wa.1.seconds())
    };
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
    reset(&mut h);
    // Screen x of a time, from the handles at 1 s and 4 s.
    let (b, e) = (rect(&h, "timeline.workArea.begin").center(), rect(&h, "timeline.workArea.end").center());
    let pps = (e.x - b.x) / 3.0;
    let x = |t: f32| b.x + (t - 1.0) * pps;
    // The end handle to 5 px before the CTI (6 s).
    let to = pos2(x(6.0) - 5.0, e.y);
    drag(&mut h, e, to);
    assert!(work_area(&h).1 < 5.99, "no Shift: no snap {:?}", work_area(&h));
    reset(&mut h);
    drag_with(&mut h, e, to, Modifiers::SHIFT);
    assert!(near(work_area(&h), (1.0, 6.0)), "the end snaps to the CTI: {:?}", work_area(&h));
    // The bar moved so its end comes 5 px before the CTI: it moves as a whole and snaps.
    reset(&mut h);
    let mid = rect(&h, "timeline.workArea.bar").center();
    let to = mid + vec2(x(6.0) - 5.0 - e.x, 0.0);
    drag(&mut h, mid, to);
    let (a, z) = work_area(&h);
    assert!((z - a - 3.0).abs() < 1e-6 && z < 5.99 && a > 1.5, "no Shift: moved, no snap {a}..{z}");
    reset(&mut h);
    drag_with(&mut h, mid, to, Modifiers::SHIFT);
    assert!(near(work_area(&h), (3.0, 6.0)), "the bar snaps its end to the CTI: {:?}", work_area(&h));
}

/// #284: pressing in the time ruler moves the current time there at once, before the pointer
/// moves or the button comes up (After Effects); dragging on then scrubs.
#[test]
fn a_press_in_the_ruler_moves_the_current_time() {
    let (mut h, _) = harness();
    // (Opening the comp showed all of it.)
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.state_mut().ui.timeline.start = 10.0;
    h.run_steps(2);
    let ruler = rect(&h, "timeline.ruler");
    let (start, _) = visible(&h);
    // 2 s into the visible span at 200 px/s (the ruler maps time from 6 px in).
    let at = pos2(ruler.min.x + 6.0 + 400.0, ruler.min.y + 17.0);
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    let now = h.state().session.time().seconds();
    assert!((now - (start + 2.0)).abs() < 0.05, "the press moved the CTI to {}: {now}", start + 2.0);
    // Still held: dragging scrubs on.
    h.event(Event::PointerMoved(at + vec2(100.0, 0.0)));
    h.step();
    let now = h.state().session.time().seconds();
    assert!((now - (start + 2.5)).abs() < 0.05, "{now}");
    h.event(Event::PointerButton { pos: at + vec2(100.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
}

/// #275: a marker dragged out of the composition marker bin, at the right end of the time ruler,
/// adds a composition marker where it is dropped, as one undo step; dragged back onto the bin,
/// none. Numpad * (typed `*`) adds one at the current time.
#[test]
fn dragging_out_of_the_marker_bin_adds_a_comp_marker() {
    let (mut h, _) = harness();
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.state_mut().ui.timeline.start = 10.0;
    h.run_steps(2);
    let markers = |h: &Harness<'_, EffectcraftApp>| -> Vec<f64> { h.state().session.active_comp().unwrap().markers.iter().map(|m| m.time.seconds()).collect() };
    let bin = rect(&h, "timeline.markerBin");
    let ruler = rect(&h, "timeline.ruler");
    assert!(ruler.contains_rect(bin) && bin.max.x > ruler.max.x - 20.0, "at the ruler's right end: {bin:?} in {ruler:?}");
    // Out to the ruler and back onto the bin: nothing.
    let at = pos2(ruler.min.x + 6.0 + 600.0, ruler.center().y);
    h.event(Event::PointerMoved(bin.center()));
    h.step();
    h.event(Event::PointerButton { pos: bin.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for p in [at, bin.center()] {
        h.event(Event::PointerMoved(p));
        h.step();
    }
    h.event(Event::PointerButton { pos: bin.center(), button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(markers(&h).is_empty(), "dropped on the bin: {:?}", markers(&h));
    // Dropped 3 s into the visible span (from 10 s at 200 px/s): a marker at 13 s.
    let undo = h.state().session.history.undo.len();
    drag(&mut h, bin.center(), at);
    assert_eq!(markers(&h), vec![13.0]);
    let history = &h.state().session.history.undo;
    assert_eq!((history.len(), history.last().map(|u| u.0.as_str())), (undo + 1, Some("Add Marker")), "one undo step");
    // Numpad * (it arrives as text) with no layer selected: Add Marker at the current time.
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    h.state_mut().session.execute("time.set", json!({"time": 20.0})).unwrap();
    h.event(Event::Text("*".into()));
    h.run_steps(2);
    assert_eq!(markers(&h), vec![13.0, 20.0]);
}

/// #976: as in After Effects, Numpad * marks the selected layers at the current time; only with
/// no layer selected does it add a composition marker.
#[test]
fn numpad_star_marks_the_selected_layer() {
    let (mut h, layer) = harness();
    let comp_markers =
        |h: &Harness<'_, EffectcraftApp>| -> Vec<f64> { h.state().session.active_comp().unwrap().markers.iter().map(|m| m.time.seconds()).collect() };
    let layer_markers = |h: &Harness<'_, EffectcraftApp>| -> Vec<f64> {
        let comp = h.state().session.active_comp().unwrap();
        comp.layer(effectcraft_engine::project::LayerId(layer)).unwrap().markers.iter().map(|m| m.time.seconds()).collect()
    };
    let star = |h: &mut Harness<'_, EffectcraftApp>, time: f64| {
        h.state_mut().session.execute("time.set", json!({"time": time})).unwrap();
        h.event(Event::Text("*".into()));
        h.run_steps(2);
    };
    h.state_mut().session.execute("layer.select", json!({"layers": [layer]})).unwrap();
    star(&mut h, 2.0);
    assert_eq!((layer_markers(&h), comp_markers(&h)), (vec![2.0], vec![]));
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    star(&mut h, 3.0);
    assert_eq!((layer_markers(&h), comp_markers(&h)), (vec![2.0], vec![3.0]));
}

/// #468: zoomed in, a CTI scrub held past the right end of the ruler scrolls the timeline (the
/// CTI rides the edge), past the left end scrolls it back; during playback the view pages to
/// keep the CTI in sight.
#[test]
fn the_timeline_follows_the_cti_past_its_edges() {
    let (mut h, _) = harness();
    // (Opening the comp showed all of it.)
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.run_steps(2);
    let ruler = rect(&h, "timeline.ruler");
    let (s0, e0) = visible(&h);
    assert_eq!(s0, 0.0);
    let y = ruler.max.y - 4.0;
    let from = pos2(ruler.center().x, y);
    let past = pos2(ruler.max.x + 60.0, y);
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=4 {
        h.event(Event::PointerMoved(from + (past - from) * (k as f32 / 4.0)));
        h.step();
    }
    // Held still past the edge: it keeps scrolling (the view is read once the button is up).
    h.run_steps(30);
    let release = |h: &mut Harness<'_, EffectcraftApp>, at: Pos2| {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
    };
    release(&mut h, past);
    let (s1, e1) = visible(&h);
    let cti = h.state().session.time().seconds();
    assert!(s1 > 0.5, "the view scrolled right: {s0}..{e0} → {s1}..{e1}");
    assert!(cti > e0 && cti >= s1 && cti <= e1 + 0.05, "the CTI rides the right edge: {cti} in {s1}..{e1}");
    // Pressed again and held past the left end: back to the left.
    let left = pos2(ruler.min.x - 60.0, y);
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=4 {
        h.event(Event::PointerMoved(from + (left - from) * (k as f32 / 4.0)));
        h.step();
    }
    h.run_steps(5);
    release(&mut h, left);
    let (s2, _) = visible(&h);
    let cti = h.state().session.time().seconds();
    assert!(s2 < s1 && (cti - s2).abs() < 0.05, "the view scrolled back left: {s1} → {s2}, CTI {cti}");

    // Playback past the visible span pages the view to the CTI.
    let (s3, e3) = visible(&h);
    let t = e3 + 20.0;
    h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(t));
    let now = h.ctx.input(|i| i.time);
    h.state_mut().play(now);
    assert!(h.state().playback.playing);
    h.step();
    h.state_mut().stop();
    h.step();
    let cti = h.state().session.time().seconds();
    let (s4, e4) = visible(&h);
    assert!(s4 > s3 && cti >= s4 && cti <= e4, "paged from {s3}..{e3} to {s4}..{e4} for the CTI at {cti}");
}
