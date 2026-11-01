//! Time zoom in the Timeline driven over the control channel, as an agent (or a person) uses it:
//! the `=` / `-` keys, the zoom-out / zoom-in buttons and the slider in the footer, and the
//! time navigator's ends. The layer bars and the Graph Editor share one time zoom (After
//! Effects): the same controls zoom the graph's time axis, centred on the current time, and
//! switching between the views keeps it.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use egui_kittest::Harness;
use serde_json::{Value, json};

/// A 10 s, 30 fps comp with a layer whose Opacity is keyed at 1 s and 8 s, revealed, the CTI at
/// 4 s; the Graph Editor on when `graph`.
fn harness(graph: bool) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Zoom", "width": 320, "height": 180, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = s.active_comp().unwrap().layers[0].id;
    for (t, v) in [(1.0, 0.0), (8.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": id.0, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [id.0]})).unwrap();
    let mut app = EffectcraftApp::new(s);
    // (Opening a comp may zoom; start from the whole comp.)
    app.ui.timeline.pps = None;
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(3);
    h.state_mut().session.execute("time.set", json!({"time": 4.0})).unwrap();
    h.state_mut().ui.timeline.pps = None;
    send(&mut h, "ui.menu.invoke", json!({"command": "timeline.reveal.opacity"}));
    if graph {
        send(&mut h, "ui.click", json!({"id": "timeline.switch.graph"}));
    }
    h.run_steps(3);
    assert_eq!(h.state().ui.timeline.graph_editor, graph);
    h
}

/// One control-channel request, then its synthetic input fed in one event per frame (kittest
/// doesn't call `raw_input_hook`).
fn send(h: &mut Harness<'_, EffectcraftApp>, method: &str, params: Value) {
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new(method, params);
    let now = h.state().now();
    if let Outcome::Done(v) = control::handle(h.state_mut(), &ctx, &req, now) {
        assert!(v["ok"] != false, "{method}: {v}");
    }
    for e in h.state_mut().take_synthetic_input() {
        h.event(e);
        h.step();
    }
    h.run_steps(3);
}

/// Visible span (start, seconds shown) from the ruler's automation label ("start,pps"); the
/// time graph ends 18 px before the ruler and maps time from 6 px in.
fn view(h: &Harness<'_, EffectcraftApp>) -> (f64, f64) {
    let e = h.state().auto.find("timeline.ruler").expect("ruler");
    let (start, pps) = e.label.split_once(',').map(|(a, b)| (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap())).unwrap();
    (start, (e.rect[2] - 24.0) as f64 / pps)
}

fn x_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> f32 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    e.rect[0] + e.rect[2] / 2.0
}

/// The span shrinks (`in`) or grows by more than 10 %, and the CTI stays where it was.
fn zoomed(h: &Harness<'_, EffectcraftApp>, before: f64, cti: f32, zoom_in: bool, what: &str) -> f64 {
    let (_, span) = view(h);
    if zoom_in {
        assert!(span < before * 0.9, "{what} zooms in: {before} -> {span}");
    } else {
        assert!(span > before * 1.1, "{what} zooms out: {before} -> {span}");
    }
    assert!((x_of(h, "timeline.cti") - cti).abs() < 1.5, "{what}: the CTI stays put ({cti} -> {})", x_of(h, "timeline.cti"));
    span
}

fn check_controls(graph: bool) {
    let mut h = harness(graph);
    let (_, span) = view(&h);
    assert!((span - 10.0).abs() < 0.2, "starts at the whole comp: {span}");
    let cti = x_of(&h, "timeline.cti");
    send(&mut h, "ui.key", json!({"key": "="}));
    let span = zoomed(&h, span, cti, true, "=");
    send(&mut h, "ui.key", json!({"key": "="}));
    let span = zoomed(&h, span, cti, true, "= again");
    send(&mut h, "ui.key", json!({"key": "-"}));
    let span = zoomed(&h, span, cti, false, "-");
    send(&mut h, "ui.click", json!({"id": "timeline.zoomInButton"}));
    let span = zoomed(&h, span, cti, true, "the zoom-in button");
    send(&mut h, "ui.click", json!({"id": "timeline.zoomOutButton"}));
    let span = zoomed(&h, span, cti, false, "the zoom-out button");
    // A click on the slider's track jumps the zoom there.
    send(&mut h, "ui.click", json!({"id": "timeline.zoomSlider", "fx": 0.9}));
    zoomed(&h, span, cti, true, "a click near the slider's right end");
    send(&mut h, "ui.click", json!({"id": "timeline.zoomSlider", "fx": 0.0}));
    assert_eq!(h.state().ui.timeline.pps, None, "the slider's left end shows the whole comp");
    // Zooming out stops at the whole comp.
    send(&mut h, "ui.click", json!({"id": "timeline.zoomOutButton"}));
    send(&mut h, "ui.key", json!({"key": "-"}));
    assert_eq!(h.state().ui.timeline.pps, None, "no further out than the whole comp");
}

#[test]
fn keys_buttons_and_slider_zoom_the_layer_bars() {
    check_controls(false);
}

#[test]
fn keys_buttons_and_slider_zoom_the_graph_editor_time_axis() {
    check_controls(true);
}

/// In the Graph Editor the curves follow the time zoom: a key's screen distance from the CTI
/// grows with `=`, and the navigator's end handle zooms the graph too.
#[test]
fn graph_curves_follow_the_time_zoom_and_the_navigator() {
    let mut h = harness(true);
    let key_x = |h: &Harness<'_, EffectcraftApp>| {
        let mut ks: Vec<f32> = h.state().auto.query("timeline.graph.key.").iter().map(|e| e.rect[0] + e.rect[2] / 2.0).collect();
        ks.sort_by(f32::total_cmp);
        ks
    };
    let ks = key_x(&h);
    assert_eq!(ks.len(), 2, "both Opacity keys in the graph: {ks:?}");
    let cti = x_of(&h, "timeline.cti");
    let d0 = ks[1] - cti;
    send(&mut h, "ui.key", json!({"key": "="}));
    let d1 = key_x(&h).last().copied().unwrap_or(cti) - cti;
    assert!(d1 > d0 * 1.3, "the 8 s key moves away from the CTI as time zooms in: {d0} -> {d1}");
    // Back to the whole comp, then the navigator's end handle dragged to the middle: 0..5 s.
    send(&mut h, "ui.menu.invoke", json!({"command": "timeline.zoomFit"}));
    let nav = h.state().auto.find("timeline.navigator").unwrap().rect;
    let end = h.state().auto.find("timeline.navigator.end").unwrap().rect;
    let y = end[1] + end[3] / 2.0;
    send(&mut h, "ui.drag", json!({"from": {"x": end[0] + end[2] / 2.0, "y": y}, "to": {"x": nav[0] + nav[2] * 0.5, "y": y}}));
    let (start, span) = view(&h);
    assert!(start.abs() < 1e-6 && (span - 5.0).abs() < 0.3, "0..5 s: {start} + {span}");
    assert_eq!(key_x(&h).len(), 1, "the 8 s key is past the visible span");
}

#[test]
fn leaving_the_graph_keeps_its_time_zoom() {
    let mut h = harness(false);
    // Opening the Graph Editor fits the graphs (ui_graph_fit); the zoom set there stays.
    send(&mut h, "ui.click", json!({"id": "timeline.switch.graph"}));
    assert!(h.state().ui.timeline.graph_editor);
    send(&mut h, "ui.key", json!({"key": "="}));
    let graph = view(&h);
    send(&mut h, "ui.click", json!({"id": "timeline.switch.graph"}));
    assert!(!h.state().ui.timeline.graph_editor);
    assert_eq!(view(&h), graph);
}

#[test]
fn azerty_physical_zoom_keys_zoom_the_graph() {
    let mut h = harness(true);
    // The "Elias AE"-style bindings: AZERTY's - (US 6 position) zooms in, ) (US minus) out.
    for (command, keys) in [("timeline.zoomIn", "Physical+6"), ("timeline.zoomOut", "Physical+Minus")] {
        h.state_mut().session.execute("shortcuts.set", json!({"command": command, "keys": keys})).unwrap();
    }
    let (_, span0) = view(&h);
    send(&mut h, "ui.key", json!({"key": "Physical+6"}));
    let (_, span1) = view(&h);
    assert!(span1 < span0 * 0.8, "Physical+6 zooms the graph in: {span0} -> {span1}");
    send(&mut h, "ui.key", json!({"key": "Physical+Minus"}));
    let (_, span2) = view(&h);
    assert!(span2 > span1 * 1.2, "Physical+Minus zooms the graph out: {span1} -> {span2}");
}

#[test]
fn zoomed_in_the_ruler_stops_at_the_comp_end_and_a_scrub_past_the_edge_scrolls() {
    let mut h = harness(false);
    for _ in 0..4 {
        send(&mut h, "ui.key", json!({"key": "="}));
    }
    // Scrolled as far right as it goes: the view ends at the comp's end, not past it (AE).
    h.state_mut().ui.timeline.start = 9.9;
    h.run_steps(3);
    let (start, span) = view(&h);
    assert!(span < 9.0, "zoomed in: {span}");
    assert!((start + span - 10.0).abs() < 0.05, "the view ends at 10 s: {start} + {span}");
    // A click at the ruler's right end puts the CTI there.
    let ruler = h.state().auto.find("timeline.ruler").unwrap().rect;
    let y = ruler[1] + ruler[3] - 4.0;
    send(&mut h, "ui.click", json!({"x": ruler[0] + ruler[2] - 26.0, "y": y}));
    let t = h.state().session.time().seconds();
    assert!(t > start + span * 0.9, "the CTI jumps to the click near the end: {t}");
    // Dragged from the ruler out over the layer outline, the view scrolls with the CTI.
    send(&mut h, "ui.drag", json!({"from": {"x": ruler[0] + 40.0, "y": y}, "to": {"x": ruler[0] - 200.0, "y": y}, "steps": 20}));
    let (start2, span2) = view(&h);
    let t2 = h.state().session.time().seconds();
    assert!(t2 < start, "the CTI moved left of the old view: {t2} vs {start}");
    assert!(t2 >= start2 - 1e-6 && t2 <= start2 + span2, "the CTI stays in view: {t2} in {start2}+{span2}");
}
