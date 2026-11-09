//! #3042: opening a migrated negative Taper length in the timeline and leaving with Tab without
//! typing writes nothing, on a key and between keys.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::{Keyframe, Value};
use effectcraft_engine::project::LayerId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn keys(h: &Harness<'_, EffectcraftApp>, layer: u64, uid: u64) -> Vec<(Tick, f64)> {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    l.props.find(uid).unwrap().keys.iter().map(|k| (k.time, k.value.as_f64())).collect()
}

fn press(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn tab_out_of_an_untouched_negative_taper_length_keeps_the_keys() {
    for at in [0.0, 0.5] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "T", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
        let r = s.execute("shape.newPath", json!({"vertices": [[20, 50], [180, 50]], "fill": false, "stroke": [1, 0, 0], "strokeWidth": 10})).unwrap();
        let layer = r["layer"].as_u64().unwrap();
        s.execute("shape.stroke.taper", json!({"layer": layer, "endLength": 10})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let uid = {
            let l = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(layer)).unwrap();
            let group = r["group"].as_u64().unwrap();
            let stroke = l.props.find_group(group).unwrap().sub("contents").unwrap().sub("stroke").unwrap();
            let uid = stroke.prop("taper/endLength").unwrap().uid;
            let p = l.props.find_mut(uid).unwrap();
            p.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(-5.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(70.0))];
            uid
        };
        s.set_time(Tick::from_seconds_f64(at));
        let mut app = EffectcraftApp::new(s);
        app.ui.timeline.open_layers.insert(layer);
        app.ui.timeline.layer_reveal.insert(layer, vec!["props".into()]);
        app.ui.timeline.reveal_props.insert(uid);
        let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
        h.run_steps(4);
        let before = keys(&h, layer, uid);
        let e = h.state().auto.find(&format!("timeline.prop.{uid}.value")).expect("the Taper End Length value is on the timeline");
        let p = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        h.input_mut().events.push(Event::PointerMoved(p));
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
        assert!(h.ctx.egui_wants_keyboard_input(), "at {at}: the field is open");
        let undo = h.state().session.history.undo.len();
        press(&mut h, Key::Tab);
        assert_eq!(keys(&h, layer, uid), before, "at {at}: Tab leaves the keys alone");
        assert_eq!(h.state().session.history.undo.len(), undo, "at {at}: nothing was written");
    }
}
