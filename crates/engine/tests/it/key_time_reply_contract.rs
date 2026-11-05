//! Omitted time bases retain existing result shapes even for valid keys outside [in, out).
use effectcraft_engine::Session;
use effectcraft_project::LayerId;
use effectcraft_time::Tick;
use serde_json::{Value, json};

const LAYER_TIME: f64 = 15.3;
const COMP_TIME: f64 = 25.8;

fn setup(text: bool) -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Reply contract", "width": 80, "height": 60, "duration": 20, "frameRate": 30})).unwrap();
    let created = if text {
        s.execute("layer.newText", json!({"name": "Document", "text": "Before"})).unwrap()
    } else {
        s.execute("layer.newSolid", json!({"name": "A", "color": "#ffffff"})).unwrap()
    };
    let layer = created["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layer": layer, "start": 10.5, "in": 10.5, "out": 20})).unwrap();
    (s, layer)
}

fn project(s: &Session) -> Value {
    serde_json::to_value(s.project.as_ref()).unwrap()
}

fn read(s: &mut Session, layer: u64, path: &str, time: f64) -> Value {
    s.execute("prop.get", json!({"layer": layer, "path": path, "time": time})).unwrap()
}

fn outside_key(s: &Session, layer: u64, path: &str, layer_time: f64, comp_time: f64) {
    let comp = s.active_comp().unwrap();
    let owner = comp.layer(LayerId(layer)).unwrap();
    let property = owner.props.prop(path).unwrap();
    let at = Tick::from_seconds_f64(layer_time);
    assert!(property.keys.iter().any(|key| key.time == at), "actual stored layer-time key");
    assert_eq!(owner.comp_time(at), Tick::from_seconds_f64(comp_time), "actual comp-time mapping");
    assert!(!owner.is_active_at(owner.comp_time(at)), "authored key is outside the active layer range");
}

fn undo_redo(s: &mut Session, before: &Value, after: &Value, history_before: usize) {
    assert_ne!(before, after, "the public edit changed the project");
    assert_eq!(s.history.undo.len(), history_before + 1, "one public edit or batch Undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(project(s), *before, "complete project Undo");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(project(s), *after, "complete project Redo");
}

fn seconds(actual: &Value, expected: f64) {
    assert!((actual.as_f64().unwrap() - expected).abs() < 1e-8, "{actual} != {expected}");
}

#[test]
fn legacy_outside_add_key_keeps_its_numeric_count_and_undo() {
    let (mut s, layer) = setup(false);
    let positive = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": 4.8, "value": 20})).unwrap();
    assert_eq!(positive.as_u64(), Some(1), "ordinary in-range numeric reply positive");
    let before = project(&s);
    let history_before = s.history.undo.len();
    let reply = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "value": 60})).unwrap();
    outside_key(&s, layer, "transform/opacity", LAYER_TIME, COMP_TIME);
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
    eprintln!("LEGACY_REPLY addKey outside: {reply}");
    assert_eq!(reply.as_u64(), Some(2), "omitted timeBase must preserve the numeric addKey count outside the active range");
}

#[test]
fn legacy_outside_scalar_set_keeps_its_value_and_undo() {
    let (mut s, layer) = setup(false);
    s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": 4.8, "value": 20})).unwrap();
    s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "value": 60})).unwrap();
    let before = project(&s);
    let history_before = s.history.undo.len();
    let reply = s.execute("prop.set", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "value": 40})).unwrap();
    outside_key(&s, layer, "transform/opacity", LAYER_TIME, COMP_TIME);
    seconds(&read(&mut s, layer, "transform/opacity", COMP_TIME)["value"], 40.0);
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
    eprintln!("LEGACY_REPLY scalar set outside: {reply}");
    assert_eq!(reply.as_f64(), Some(40.0), "omitted timeBase must preserve the scalar set value outside the active range");
}

#[test]
fn legacy_outside_selection_keeps_its_numeric_count_without_an_edit() {
    let (mut s, layer) = setup(false);
    s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "value": 60})).unwrap();
    outside_key(&s, layer, "transform/opacity", LAYER_TIME, COMP_TIME);
    let before = project(&s);
    let history_before = s.history.undo.len();
    let reply = s.execute("keys.select", json!({"keys": [{"layer": layer, "path": "transform/opacity", "time": LAYER_TIME}]})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 1, "actual owner selected");
    assert_eq!(s.state.selected_keys[0].layer, LayerId(layer));
    assert_eq!(s.state.selected_keys[0].time, Tick::from_seconds_f64(LAYER_TIME));
    assert_eq!(project(&s), before);
    assert_eq!(s.history.undo.len(), history_before);
    eprintln!("LEGACY_REPLY select outside: {reply}");
    assert_eq!(reply.as_u64(), Some(1), "omitted timeBase must preserve the numeric selection count outside the active range");
}

#[test]
fn legacy_outside_position_set_keeps_its_array_value_and_undo() {
    let (mut s, layer) = setup(false);
    s.execute("prop.addKey", json!({"layer": layer, "path": "transform/position", "time": LAYER_TIME, "value": [10, 20, 0]})).unwrap();
    let before = project(&s);
    let history_before = s.history.undo.len();
    let reply = s.execute("prop.set", json!({"layer": layer, "path": "transform/position", "time": LAYER_TIME, "value": [12, 34, 0]})).unwrap();
    outside_key(&s, layer, "transform/position", LAYER_TIME, COMP_TIME);
    let actual = read(&mut s, layer, "transform/position", COMP_TIME)["value"].clone();
    let values = actual.as_array().unwrap();
    assert_eq!(values.len(), 3, "ordinary Position value remains a vector");
    seconds(&values[0], 12.0);
    seconds(&values[1], 34.0);
    seconds(&values[2], 0.0);
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
    eprintln!("LEGACY_REPLY array set outside: {reply}");
    assert_eq!(reply, actual, "omitted timeBase must return the raw array value rather than a timing wrapper");
}

#[test]
fn legacy_outside_text_set_keeps_its_string_value_and_undo() {
    let (mut s, layer) = setup(true);
    let source = read(&mut s, layer, "text/sourceText", 10.5)["value"].clone();
    assert_eq!(source.as_str(), Some("Before"), "public Source Text returns its text string");
    s.execute("prop.addKey", json!({"layer": layer, "path": "text/sourceText", "time": LAYER_TIME, "value": source})).unwrap();
    let before = project(&s);
    let history_before = s.history.undo.len();
    let reply = s.execute("prop.set", json!({"layer": layer, "path": "text/sourceText", "time": LAYER_TIME, "value": "Outside final"})).unwrap();
    outside_key(&s, layer, "text/sourceText", LAYER_TIME, COMP_TIME);
    let actual = read(&mut s, layer, "text/sourceText", COMP_TIME)["value"].clone();
    assert_eq!(actual.as_str(), Some("Outside final"), "actual Source Text edit");
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
    eprintln!("LEGACY_REPLY text set outside: {reply}");
    assert_eq!(reply, actual, "omitted timeBase must return the raw Source Text string unchanged");
}

#[test]
fn legacy_outside_count_remains_usable_by_the_documented_batch_reference() {
    let (mut s, layer) = setup(false);
    let before = project(&s);
    let history_before = s.history.undo.len();
    let outcome = s.execute(
        "engine.batch",
        json!({"label": "Use key count", "steps": [
            {"command": "prop.addKey", "params": {"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "value": 60}},
            {"command": "prop.set", "params": {"layer": layer, "path": "transform/rotation", "value": "$1"}}
        ]}),
    );
    eprintln!("LEGACY_REPLY batch count substitution: {outcome:?}");
    assert!(outcome.is_ok(), "a valid outside addKey count must remain usable as the documented $1 numeric value: {outcome:?}");
    let reply = outcome.unwrap();
    assert_eq!(reply["steps"].as_u64(), Some(2));
    assert_eq!(reply["results"][0].as_u64(), Some(1));
    assert_eq!(reply["results"][1].as_f64(), Some(1.0));
    seconds(&read(&mut s, layer, "transform/rotation", 10.5)["value"], 1.0);
    outside_key(&s, layer, "transform/opacity", LAYER_TIME, COMP_TIME);
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
}

#[test]
fn explicit_time_bases_keep_timing_and_existing_object_warnings() {
    let (mut s, layer) = setup(false);
    let outside = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "timeBase": "layer", "value": 60})).unwrap();
    assert_eq!(outside["count"].as_u64(), Some(1));
    seconds(&outside["layerTime"], LAYER_TIME);
    seconds(&outside["compTime"], COMP_TIME);
    assert!(outside["warning"].as_str().unwrap().contains("outside"));
    let inside = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": 15.3, "timeBase": "comp", "value": 20})).unwrap();
    seconds(&inside["layerTime"], 4.8);
    seconds(&inside["compTime"], 15.3);
    assert_eq!(inside["count"].as_u64(), Some(2));
    assert!(inside.get("warning").is_none());
    let selected = s.execute("keys.select", json!({"keys": [{"layer": layer, "path": "transform/opacity", "time": COMP_TIME}], "timeBase": "comp"})).unwrap();
    assert_eq!(selected["count"].as_u64(), Some(1));
    seconds(&selected["keys"][0]["compTime"], COMP_TIME);
    assert!(selected["warning"].as_str().unwrap().contains("outside"));
    let changed = s.execute("prop.set", json!({"layer": layer, "path": "transform/opacity", "time": COMP_TIME, "timeBase": "comp", "value": 40})).unwrap();
    seconds(&changed["value"], 40.0);
    seconds(&changed["compTime"], COMP_TIME);
    assert!(changed["warning"].as_str().unwrap().contains("outside"));
    let info = read(&mut s, layer, "transform/opacity", COMP_TIME);
    let keys = info["keys"].as_array().unwrap();
    let key = keys.iter().find(|key| (key["time"].as_f64().unwrap() - LAYER_TIME).abs() < 1e-8).unwrap();
    seconds(&key["layerTime"], LAYER_TIME);
    seconds(&key["compTime"], COMP_TIME);
    assert!(key["warning"].as_str().unwrap().contains("outside"), "prop.get already has an object response");
    let before = project(&s);
    let history_before = s.history.undo.len();
    let moved = s.execute("keys.set", json!({"layer": layer, "path": "transform/opacity", "time": LAYER_TIME, "newTime": 15.4})).unwrap();
    assert!(moved.is_object(), "keys.set already returned an object without timeBase");
    seconds(&moved["layerTime"], 15.4);
    seconds(&moved["compTime"], 25.9);
    assert!(moved["warning"].as_str().unwrap().contains("outside"));
    outside_key(&s, layer, "transform/opacity", 15.4, 25.9);
    let after = project(&s);
    undo_redo(&mut s, &before, &after, history_before);
}
