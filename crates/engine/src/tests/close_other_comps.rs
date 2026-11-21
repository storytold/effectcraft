//! `comp.closeOthers` (Close Other Timeline Panels): one composition keeps its open Timeline and
//! becomes the active one; the others close but stay in the project (#3563).

use serde_json::json;

use crate::Session;

fn comp(s: &mut Session, name: &str) -> u64 {
    s.execute_checked("comp.new", json!({"name": name, "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap()["comp"].as_u64().unwrap()
}

#[test]
fn close_others_keeps_one_timeline_and_every_composition() {
    let mut s = Session::default();
    let (a, b, c) = (comp(&mut s, "A"), comp(&mut s, "B"), comp(&mut s, "C"));
    assert_eq!(s.state.open_comps.len(), 3, "all three open");
    assert!(s.is_enabled("comp.closeOthers"));
    // Closing others from a tab that is not the active one keeps that tab and makes it active.
    let r = s.execute("comp.closeOthers", json!({"comp": b})).unwrap();
    assert_eq!(r["closed"], 2);
    assert_eq!(s.state.open_comps.iter().map(|c| c.0).collect::<Vec<_>>(), vec![b]);
    assert_eq!(s.state.active_comp.map(|c| c.0), Some(b));
    for id in [a, b, c] {
        assert!(s.project.items.keys().any(|k| k.0 == id), "composition {id} stays in the project");
    }
    // Nothing else is open: the command is not available.
    assert!(!s.is_enabled("comp.closeOthers"));
}

#[test]
fn close_others_on_a_comp_that_is_not_open_opens_it_alone() {
    let mut s = Session::default();
    let (a, b) = (comp(&mut s, "A"), comp(&mut s, "B"));
    s.execute("comp.close", json!({"comp": a})).unwrap();
    assert_eq!(s.state.open_comps.iter().map(|c| c.0).collect::<Vec<_>>(), vec![b]);
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    let r = s.execute("comp.closeOthers", json!({"comp": a})).unwrap();
    assert_eq!(r["closed"], 2);
    assert_eq!(s.state.open_comps.iter().map(|c| c.0).collect::<Vec<_>>(), vec![a]);
    assert_eq!(s.state.active_comp.map(|c| c.0), Some(a));
}
