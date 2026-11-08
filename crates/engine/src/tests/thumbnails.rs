//! #3271: Project panel thumbnails are keyed by what they show, not by the project's revision:
//! an edit elsewhere keeps a comp's thumbnail, an edit of the comp, another time or size, and a
//! footage reload give it a new key.

use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

#[test]
fn thumbnail_keys_follow_the_content() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 64, "height": 32, "frameRate": 24, "duration": 2})).unwrap();
    let a = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "S", "color": "#ff8000"})).unwrap();
    let key = |s: &Session| s.thumbnail_key(a, Tick::ZERO, 192).unwrap();
    let k0 = key(&s);
    // Another comp: the project changed, this comp's picture didn't.
    s.execute("comp.new", json!({"name": "B", "width": 16, "height": 16, "frameRate": 24, "duration": 1})).unwrap();
    assert_eq!(key(&s), k0, "an edit elsewhere keeps the key");
    assert_ne!(s.thumbnail_key(a, Tick::from_seconds_f64(0.5), 192).unwrap(), k0, "another time");
    assert_ne!(s.thumbnail_key(a, Tick::ZERO, 96).unwrap(), k0, "another size");
    // An edit of the comp itself.
    std::sync::Arc::make_mut(&mut s.project).comp_mut(a).unwrap().background = [0.2, 0.4, 0.6];
    assert_ne!(key(&s), k0, "an edit of the comp gives a new key");
    // A reload: files may have changed without the project knowing.
    let before = key(&s);
    s.forget_decoded(&effectcraft_project::Footage::default());
    assert_ne!(key(&s), before, "a reload gives a new key");
}
