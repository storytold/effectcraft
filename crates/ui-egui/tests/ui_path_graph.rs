//! Path keyframes in the Graph Editor: temporal morph timing, without editing shape geometry.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::{ShapePath, Value, path_timing};
use effectcraft_engine::project::LayerId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, LayerId, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Path graph", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let lid = LayerId(s.execute("layer.newShape", json!({"name": "Morph"})).unwrap()["layer"].as_u64().unwrap());
    let group = s.execute("layer.addShapeItem", json!({"layer": lid.0, "kind": "path"})).unwrap()["uid"].as_u64().unwrap();
    let uid = s.active_comp().unwrap().layer(lid).unwrap().props.find_group(group).unwrap().get("path").unwrap().uid;
    for (time, dx) in [(0.5, 0.0), (2.5, 100.0)] {
        let path = ShapePath::polygon(&[[dx, 0.0], [dx + 40.0, 0.0], [dx + 20.0, 40.0]], true);
        s.execute("prop.addKey", json!({"layer": lid.0, "prop": uid, "time": time, "value": path})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [lid.0]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.animated", json!({})).unwrap();
    h.run_steps(4);
    (h, lid, uid)
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let element = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    pos2(element.rect[0] + element.rect[2] / 2.0, element.rect[1] + element.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for i in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn values(h: &Harness<'_, EffectcraftApp>, lid: LayerId, uid: u64) -> Vec<Value> {
    h.state().session.active_comp().unwrap().layer(lid).unwrap().props.find(uid).unwrap().keys.iter().map(|k| k.value.clone()).collect()
}

fn open_graph(h: &mut Harness<'_, EffectcraftApp>, mode: &str) {
    // Opening the Graph Editor fits the selected keys. Open it before the test's key
    // selection so the whole animation stays visible for an interaction across both keys.
    h.state_mut().ui.timeline.graph_editor = true;
    h.state_mut().ui.timeline.graph_mode = mode.into();
    h.run_steps(4);
}

#[test]
fn a_path_key_clicked_in_the_timeline_shows_its_morph_speed_graph() {
    let (mut h, lid, uid) = harness();
    // Timeline automation ids use frame numbers, whereas Graph Editor ids use key indices.
    let mut timeline_keys: Vec<Pos2> = h
        .state()
        .auto
        .query(&format!("timeline.key.{uid}."))
        .iter()
        .map(|element| pos2(element.rect[0] + element.rect[2] / 2.0, element.rect[1] + element.rect[3] / 2.0))
        .collect();
    timeline_keys.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(timeline_keys.len(), 2, "both Path keyframes are visible in the Timeline");
    let key = timeline_keys[1];
    click(&mut h, key);
    assert_eq!(h.state().session.state.selected_props, vec![(lid, uid)]);
    h.state_mut().ui.timeline.graph_editor = true;
    h.run_steps(4);
    assert!(h.state().auto.find(&format!("timeline.graph.key.{uid}.0.1")).is_some());
    assert_eq!(h.state().auto.find("timeline.graph.units").unwrap().label, "Path morph speed (%/s)");
    // A control-channel key selection also shows the graph, without separately selecting
    // a property or layer (keys.select does not always request selectProperties).
    h.state_mut().session.state.selected_props.clear();
    h.state_mut().session.state.selected_layers.clear();
    h.run_steps(4);
    assert!(h.state().auto.find(&format!("timeline.graph.key.{uid}.0.1")).is_some());
}

#[test]
fn path_progress_keys_drag_in_time_without_changing_geometry() {
    let (mut h, lid, uid) = harness();
    open_graph(&mut h, "value");
    h.state_mut().session.execute("keys.select", json!({"keys": [{"layer": lid.0, "prop": uid, "time": 2.5}]})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().auto.find("timeline.graph.units").unwrap().label, "Path animation progress (%)");
    let first = center(&h, &format!("timeline.graph.key.{uid}.0.0"));
    let last = center(&h, &format!("timeline.graph.key.{uid}.0.1"));
    let before = values(&h, lid, uid);
    let pps = (last.x - first.x) / 2.0;
    drag(&mut h, last, pos2(last.x + pps * 0.5, last.y + 20.0));
    let property = h.state().session.active_comp().unwrap().layer(lid).unwrap().props.find(uid).unwrap();
    assert!((property.keys[1].time.seconds() - 3.0).abs() <= 2.0 / 30.0, "Path keys follow a time drag");
    assert_eq!(values(&h, lid, uid), before, "vertical drags never alter Path geometry");
}

#[test]
fn path_speed_handle_edits_stored_progress_speed_and_influence() {
    let (mut h, lid, uid) = harness();
    open_graph(&mut h, "speed");
    h.state_mut().session.execute("keys.select", json!({"keys": [{"layer": lid.0, "prop": uid, "time": 0.5}]})).unwrap();
    h.run_steps(4);
    let first = center(&h, &format!("timeline.graph.key.{uid}.0.0"));
    let last = center(&h, &format!("timeline.graph.key.{uid}.0.1"));
    let handle = center(&h, &format!("timeline.graph.handle.{uid}.0.0.out"));
    let before = values(&h, lid, uid);
    drag(&mut h, handle, pos2(handle.x + (last.x - first.x) * 0.1, handle.y - 20.0));
    let property = h.state().session.active_comp().unwrap().layer(lid).unwrap().props.find(uid).unwrap();
    let ease = property.keys[0].out_ease[0];
    assert!((ease.influence - (1.0 / 3.0 + 0.1)).abs() < 0.02, "{ease:?}");
    assert!(ease.speed > 0.5 && ease.speed < 2.0, "UI percentages convert back to progress/s: {ease:?}");
    assert_eq!(values(&h, lid, uid), before);
    let progress = path_timing(&property.keys, Tick::from_seconds_f64(1.5)).unwrap().0;
    let geometry = effectcraft_engine::keyframe::evaluate(&property.keys, Tick::from_seconds_f64(1.5), false).unwrap();
    assert!((geometry.as_path().unwrap().vertices[0][0] / 100.0 - progress).abs() < 1e-10);
    if let Ok(path) = std::env::var("PATH_GRAPH_REVIEW_PNG") {
        h.render().unwrap().save(path).unwrap();
    }
}

#[test]
fn stretched_and_reversed_path_handles_follow_comp_time_and_keep_easing() {
    for (percent, reverse) in [(100.0, false), (50.0, false), (200.0, false), (100.0, true)] {
        let (mut h, lid, uid) = harness();
        let params = if reverse { json!({"layers": [lid.0], "op": "reverse"}) } else { json!({"layers": [lid.0], "percent": percent}) };
        h.state_mut().session.execute("layer.timeStretch", params).unwrap();
        open_graph(&mut h, "speed");
        h.state_mut().session.execute("keys.select", json!({"keys": [{"layer": lid.0, "prop": uid, "time": 0.5}]})).unwrap();
        h.state_mut().ui.timeline.pps = Some(100.0);
        h.run_steps(4);
        let first = center(&h, &format!("timeline.graph.key.{uid}.0.0"));
        let last = center(&h, &format!("timeline.graph.key.{uid}.0.1"));
        let handle = center(&h, &format!("timeline.graph.handle.{uid}.0.0.out"));
        assert!(((handle.x - first.x) / (last.x - first.x) - 1.0 / 3.0).abs() < 0.01, "the handle uses composition time, including reversal");
        let before = values(&h, lid, uid);
        drag(&mut h, handle, pos2(handle.x + (last.x - first.x) * 0.1, handle.y));
        let property = h.state().session.active_comp().unwrap().layer(lid).unwrap().props.find(uid).unwrap();
        let ease = property.keys[0].out_ease[0];
        assert!((ease.influence - (1.0 / 3.0 + 0.1)).abs() < 0.02, "{ease:?}");
        assert!((ease.speed - 0.5).abs() < 1e-6, "stretch changes graph units, not stored native speed: {ease:?}");
        assert_eq!(values(&h, lid, uid), before);
    }
}

#[test]
fn timeline_stretch_column_shows_the_engine_percentage() {
    let (mut h, lid, _) = harness();
    h.state_mut().ui.timeline.columns.insert("stretch".into());
    h.run_steps(3);
    let id = format!("timeline.layer.{}.stretch", lid.0);
    assert_eq!(h.state().auto.find(&id).unwrap().label, "100.0%");
    h.state_mut().session.execute("layer.timeStretch", json!({"layers": [lid.0], "percent": 200})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().auto.find(&id).unwrap().label, "200.0%");
}
