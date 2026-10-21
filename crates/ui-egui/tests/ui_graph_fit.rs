//! Opening the Graph Editor frames the graphs, as in After Effects: Fit Selection to View when
//! keys are selected (a key selected in the Timeline shows its property's graph even when the
//! layer isn't selected), else Fit All Graphs to View. It used to open on an empty 0–100 range
//! or with the curves off-view.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

/// A 20 s comp whose Box layer has Rotation keys 300° at 8 s and 720° at 12 s; the Timeline is
/// zoomed in on 0–4 s, so neither key is in view. Returns the harness and Rotation's uid.
fn harness(select_layer: bool) -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Fit", "width": 320, "height": 180, "frameRate": 30, "duration": 20})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    let id = s.active_comp().unwrap().layers[0].id;
    for (t, v) in [(8.0, 300.0), (12.0, 720.0)] {
        s.execute("prop.addKey", json!({"layer": id.0, "path": "transform/rotation", "time": t, "value": v})).unwrap();
    }
    let uid = s.active_comp().unwrap().layer(id).unwrap().props.prop("transform/rotation").unwrap().uid;
    s.execute("layer.select", json!({"layers": if select_layer { vec![id.0] } else { vec![] }})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.pps = Some(300.0);
    h.state_mut().ui.timeline.start = 0.0;
    h.run_steps(3);
    (h, id.0, uid)
}

fn open_graph(h: &mut Harness<'_, EffectcraftApp>) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.graphEditor", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().ui.timeline.graph_editor);
}

/// Every key of the property sits inside the plot, and the value range spans `lo..=hi`.
fn assert_framed(h: &Harness<'_, EffectcraftApp>, uid: u64, keys: &[usize], lo: f64, hi: f64) {
    let plot = h.state().auto.find("timeline.graph").expect("graph plot").rect;
    for i in keys {
        let k = h.state().auto.find(&format!("timeline.graph.key.{uid}.0.{i}")).unwrap_or_else(|| panic!("graph key {i} not drawn")).rect;
        let (cx, cy) = (k[0] + k[2] / 2.0, k[1] + k[3] / 2.0);
        assert!(cx > plot[0] && cx < plot[0] + plot[2] && cy > plot[1] && cy < plot[1] + plot[3], "key {i} at ({cx}, {cy}) outside plot {plot:?}");
    }
    let (a, b) = h.state().ui.timeline.graph_range.expect("graph range");
    assert!(a <= lo && b >= hi, "value range {a}..{b} doesn't contain {lo}..{hi}");
}

#[test]
fn opening_the_graph_editor_fits_the_selected_key() {
    let (mut h, layer, uid) = harness(false);
    h.state_mut().session.execute("keys.select", json!({"keys": [{"layer": layer, "prop": uid, "time": 12.0}]})).unwrap();
    h.run_steps(2);
    open_graph(&mut h);
    // A lone key also frames its neighbour, so its curve shows.
    assert_framed(&h, uid, &[0, 1], 300.0, 720.0);
    assert!(!h.state().ui.timeline.graph_auto_zoom, "Fit Selection turns auto-zoom off");
}

#[test]
fn opening_the_graph_editor_with_no_keys_selected_fits_all_graphs() {
    let (mut h, _, uid) = harness(true);
    open_graph(&mut h);
    assert_eq!(h.state().ui.timeline.pps, None, "the whole comp in time");
    assert_framed(&h, uid, &[0, 1], 300.0, 720.0);
    // Zooming away and toggling the Graph Editor off and on frames it again.
    h.state_mut().ui.timeline.pps = Some(300.0);
    h.state_mut().ui.timeline.start = 0.0;
    open_graph_toggle_twice(&mut h);
    assert_framed(&h, uid, &[0, 1], 300.0, 720.0);
}

fn open_graph_toggle_twice(h: &mut Harness<'_, EffectcraftApp>) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.graphEditor", json!({})).unwrap();
    h.run_steps(2);
    assert!(!h.state().ui.timeline.graph_editor);
    open_graph(h);
}
