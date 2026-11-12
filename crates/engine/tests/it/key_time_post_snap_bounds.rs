//! A checked comp time must remain supported after nearest-frame rounding.
use effectcraft_engine::Session;
use effectcraft_project::LayerId;
use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

fn project(s: &Session) -> Value {
    serde_json::to_value(s.project.as_ref()).unwrap()
}

fn stored_comp_time(s: &Session, layer: u64, value: f64) -> Tick {
    let owner = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    let property = owner.props.prop("transform/opacity").unwrap();
    let key = property.keys.iter().find(|key| key.value.to_json().as_f64() == Some(value)).unwrap();
    owner.comp_time(key.time)
}

fn check_bound(upper: bool) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Post-snap bound", "width": 80, "height": 60, "duration": 20, "frameRate": 25})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "A", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let start = if upper { 10.0 } else { -10.0 };
    s.execute("layer.timing", json!({"layer": layer, "start": start})).unwrap();
    let owner = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    assert_eq!(owner.start_time, Tick::from_seconds_f64(start), "ordinary frame-aligned start");
    assert_eq!(owner.stretch, 100.0);

    let bound = if upper { Tick::MAX } else { Tick::MIN };
    let nearby = if upper { bound - Tick(TICKS_PER_SECOND) } else { bound + Tick(TICKS_PER_SECOND) };
    let control_history = s.history.undo.len();
    let control =
        s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": nearby.seconds(), "timeBase": "comp", "value": 25})).unwrap();
    assert_eq!(control["count"].as_u64(), Some(1), "nearby in-range key succeeds");
    let actual_control = stored_comp_time(&s, layer, 25.0);
    assert!((Tick::MIN..=Tick::MAX).contains(&actual_control), "actual nearby comp time remains supported");
    assert!(
        actual_control.0.abs_diff(nearby.0) < (TICKS_PER_SECOND / 25) as u64,
        "nearby control remains within one ordinary 25fps frame of its requested time"
    );
    assert_eq!(s.history.undo.len(), control_history + 1, "control records one edit");
    eprintln!("BOUND_NEARBY_CONTROL upper={upper} requested={} actual={} reply={control}", nearby.0, actual_control.0);

    let before = project(&s);
    let history_before = s.history.undo.len();
    let redo_before = s.history.redo.len();
    let outcome = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": bound.seconds(), "timeBase": "comp", "value": 75}));
    if let Ok(reply) = &outcome {
        assert_eq!(reply["count"].as_u64(), Some(2), "the accepted boundary call added its key");
        let actual = stored_comp_time(&s, layer, 75.0);
        let exceeds = if upper { actual > Tick::MAX } else { actual < Tick::MIN };
        assert!(exceeds, "an accepted boundary call must prove the predicted outward comp-time snap");
        assert_eq!(s.history.undo.len(), history_before + 1, "the accepted boundary call is an actual edit");
        eprintln!(
            "BOUND_OUTWARD_ACCEPTED upper={upper} requested={} actual={} history_before={} history_after={} reply={reply}",
            bound.0,
            actual.0,
            history_before,
            s.history.undo.len()
        );
    }
    assert!(outcome.is_err(), "nearest-frame rounding beyond the supported comp-time bound must reject before an edit: upper={upper}, outcome={outcome:?}");
    let error = outcome.unwrap_err().to_string();
    assert!(error.contains("converts outside the supported time range"), "contextual conversion error: {error}");
    assert_eq!(project(&s), before, "rejected post-snap time leaves the full project unchanged");
    assert_eq!(s.history.undo.len(), history_before, "rejected post-snap time records no Undo step");
    assert_eq!(s.history.redo.len(), redo_before, "rejected post-snap time leaves Redo unchanged");
}

#[test]
fn upper_supported_comp_time_is_rejected_after_outward_frame_snap() {
    check_bound(true);
}

#[test]
fn lower_supported_comp_time_is_rejected_after_outward_frame_snap() {
    check_bound(false);
}
