use effectcraft_project::LayerId;
use effectcraft_time::Tick;
use serde_json::json;

use super::timeline::animate;
use crate::Session;

#[test]
fn freeze_on_last_frame_preserves_reversed_playback_before_the_hold() {
    check_freeze_on_last_frame(true, 0.0);
}

#[test]
fn freeze_on_last_frame_preserves_reversed_playback_on_an_offset_layer() {
    check_freeze_on_last_frame(true, 0.5);
}

#[test]
fn freeze_on_last_frame_preserves_forward_playback_before_the_hold() {
    check_freeze_on_last_frame(false, 0.0);
}

fn check_freeze_on_last_frame(reversed: bool, offset: f64) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let solid = session.execute("layer.newSolid", json!({"color": "#ffffff", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    animate(&mut session, solid, "transform/position", &[(0.0, json!([20, 50])), (2.0, json!([80, 50]))]);
    session.execute("comp.new", json!({"name": "Outer", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
    let comp_id = session.active_comp_id().unwrap();
    let layer_id = LayerId(session.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap());
    if offset != 0.0 {
        session.execute("layer.timing", json!({"layers": [layer_id.0], "delta": offset})).unwrap();
    }
    if reversed {
        session.execute("layer.timeReverse", json!({"layers": [layer_id.0]})).unwrap();
    }

    let comp = session.active_comp().unwrap();
    let layer = comp.layer(layer_id).unwrap();
    let (original_in, original_out, original_start, original_stretch) = (layer.in_point, layer.out_point, layer.start_time, layer.stretch);
    assert_eq!(original_in, Tick::from_seconds_f64(offset));
    assert_eq!(original_stretch, if reversed { -100.0 } else { 100.0 });
    let frame_duration = comp.frame_duration();
    let comp_end = comp.duration;
    let render = |session: &Session, time| session.render(comp_id, time, Default::default());
    let playback_times = [original_in, original_in + comp.frame_rate.tick_of(15), original_in + comp.frame_rate.tick_of(30), original_out - frame_duration];
    let before = playback_times.map(|time| render(&session, time));
    let extension_times = [original_out, comp_end - frame_duration];
    let before_extension = extension_times.map(|time| render(&session, time));
    let at_comp_end = render(&session, comp_end);
    let last_frame = before.last().unwrap();
    assert!(before.windows(2).all(|pair| pair[0] != pair[1]));
    assert!(before_extension.iter().all(|frame| frame != last_frame));
    assert!(at_comp_end != *last_frame);

    let check_playback = |session: &Session, operation| {
        for (time, frame) in playback_times.into_iter().zip(&before) {
            assert!(render(session, time) == *frame, "{operation} changed the frame at {} s", time.seconds());
        }
    };

    session.execute("layer.freezeOnLastFrame", json!({"layers": [layer_id.0]})).unwrap();
    check_playback(&session, "Freeze On Last Frame");
    for time in extension_times {
        assert!(render(&session, time) == *last_frame, "the extension should hold the last frame at {} s", time.seconds());
    }
    assert_eq!(session.active_comp().unwrap().layer(layer_id).unwrap().out_point, comp_end);
    assert!(render(&session, comp_end) == at_comp_end, "the layer should end at the composition's exclusive out point");

    session.execute("edit.undo", json!({})).unwrap();
    let restored = session.active_comp().unwrap().layer(layer_id).unwrap();
    assert!(restored.props.get("timeRemap").is_none());
    assert_eq!((restored.in_point, restored.out_point, restored.start_time, restored.stretch), (original_in, original_out, original_start, original_stretch));
    check_playback(&session, "Undo");
    for (time, frame) in extension_times.into_iter().zip(&before_extension) {
        assert!(render(&session, time) == *frame, "Undo should remove the held frame at {} s", time.seconds());
    }
}
