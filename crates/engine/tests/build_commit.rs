//! `app.capabilities` names the commit the build came from, so agents and bug reports can tell a
//! release from a local build of `main` that reports the same version number.
use effectcraft_engine::{BUILD_COMMIT, Session};
use serde_json::json;

fn is_commit(c: &str) -> bool {
    c == "unknown" || (c.len() >= 7 && c.chars().all(|ch| ch.is_ascii_hexdigit()))
}

#[test]
fn capabilities_report_the_build_commit() {
    let mut s = Session::default();
    let caps = s.execute("app.capabilities", json!({})).unwrap();
    assert_eq!(caps["commit"], json!(BUILD_COMMIT));
    assert!(is_commit(BUILD_COMMIT), "not a commit id: {BUILD_COMMIT:?}");
    assert_eq!(caps["version"], json!(env!("CARGO_PKG_VERSION")));
}
