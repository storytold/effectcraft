use serde_json::json;

use super::timeline::animate;
use crate::Session;
use effectcraft_time::Tick;

#[test]
fn freeze_on_last_frame_preserves_reversed_playback_before_the_hold() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let solid = session.execute("layer.newSolid", json!({"color": "#ffffff", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    animate(&mut session, solid, "transform/position", &[(0.0, json!([20, 50])), (2.0, json!([80, 50]))]);
    session.execute("comp.new", json!({"name": "Outer", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
    let comp = session.active_comp_id().unwrap();
    let layer = session.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap();
    session.execute("layer.timeReverse", json!({"layers": [layer]})).unwrap();

    let render = |session: &Session, time| session.render(comp, Tick::from_seconds_f64(time), Default::default());
    let times = [0.0, 0.5, 1.0, 59.0 / 30.0];
    let before = times.map(|time| render(&session, time));
    assert!(before.windows(2).all(|pair| pair[0] != pair[1]));

    session.execute("layer.freezeOnLastFrame", json!({"layers": [layer]})).unwrap();
    for (time, frame) in times.into_iter().zip(&before) {
        assert!(render(&session, time) == *frame, "Freeze On Last Frame changed the frame at {time} s");
    }
    assert!(render(&session, 3.5) == before[3], "the extension should hold the last reversed frame");

    session.undo();
    let restored = session.active_comp().unwrap().layers.first().unwrap();
    assert!(restored.props.get("timeRemap").is_none());
    assert_eq!(restored.out_point, Tick::from_seconds_f64(2.0));
}
