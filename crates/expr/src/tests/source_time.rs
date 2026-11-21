//! `Layer.sourceTime(t)`: the layer's source time at comp time `t`, through its start, stretch and
//! Time Remap, as the layer is drawn. Rigs call it on another layer (Karton:
//! `baseLayer.sourceRectAtTime(baseLayer.sourceTime(t))`); it was missing, so the call threw
//! "not a callable function" inside their `try` and the rig fell back to stored values.

use super::*;
use effectcraft_project::Property;

#[test]
fn source_time_follows_start_stretch_and_time_remap() {
    let mut f = fx();
    let (a, b) = (f.a, f.b);
    // A: starts at 2 s. B: stretched to 200 %.
    f.layer_mut(a).start_time = secs(2.0);
    f.layer_mut(b).stretch = 200.0;
    assert_close!(f.num("thisComp.layer('Title').sourceTime(5)", 0.0), 5.0);
    assert_close!(f.num("thisComp.layer('A').sourceTime(5)", 0.0), 3.0);
    assert_close!(f.num("thisComp.layer('B').sourceTime(4)", 0.0), 2.0);
    // The default time is the current time; a property's value counts as a time.
    assert_close!(f.num("thisComp.layer('Title').sourceTime()", 1.5), 1.5);
    assert_close!(f.num("thisComp.layer('Title').sourceTime(thisComp.layer('A').effect('Slider Control')('Slider'))", 0.0), 42.0);
    // Time Remap: its value at that time (keys 0 s → 10 s and 1 s → 0 s, a reversed second). Its
    // keys are in layer time, which B's 200 % stretch halves: comp 0.25 s is layer 0.125 s.
    let uid = f.p.next_id;
    f.p.next_id += 1;
    let mut tr = Property::new(uid, "timeRemap", "Time Remap", Value::Scalar(0.0));
    tr.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(10.0)), Keyframe::new(secs(1.0), Value::Scalar(0.0))];
    f.layer_mut(b).props.children.insert(0, tr.into());
    assert_close!(f.num("thisComp.layer('B').sourceTime(0.25)", 0.0), 8.75);
}
