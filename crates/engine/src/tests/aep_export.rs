//! File ▸ Export ▸ After Effects Project… through the command registry.

use serde_json::json;

use crate::Session;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-aep-export-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 25, "duration": 2})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff8000", "width": 100, "height": 60})).unwrap();
    s.execute("prop.addKey", json!({"layer": "#1", "path": "transform/opacity", "time": 0.0, "value": 0.0})).unwrap();
    s.execute("prop.addKey", json!({"layer": "#1", "path": "transform/opacity", "time": 1.0, "value": 100.0})).unwrap();
    s
}

#[test]
fn export_aep_writes_a_file_that_imports_back() {
    let mut s = session();
    let path = tmp("main.aep");
    let undo_before = s.history.undo.len();
    let r = s.execute_checked("file.exportAep", json!({"path": path})).unwrap();
    assert!(r["bytes"].as_u64().unwrap() > 1000);
    assert_eq!(r["report"]["exported"]["compositions"], 1);
    assert_eq!(r["report"]["exported"]["keyframes"], 2);
    assert_eq!(r["report"]["losses"], json!([]));
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"RIFX"));
    // Export does not change the project (no undo step).
    assert_eq!(s.history.undo.len(), undo_before);
    let back = s.execute_checked("file.importAep", json!({"path": path})).unwrap();
    assert_eq!(back["report"]["imported"]["compositions"], 1);
    assert_eq!(back["report"]["imported"]["keyframes"], 2);
    assert_eq!(back["report"]["losses"], json!([]));
}

#[test]
fn losses_are_refused_unless_allowed_and_a_dry_run_only_reports() {
    let mut s = session();
    s.execute("layer.newText", json!({})).ok();
    // A parallel light is not written (no sampled case).
    s.execute("layer.newLight", json!({"kind": "Parallel"})).ok();
    let path = tmp("lossy.aep");
    let _ = std::fs::remove_file(&path);
    let err = s.execute_checked("file.exportAep", json!({"path": path})).unwrap_err().to_string();
    assert!(err.contains("allowLosses"), "{err}");
    assert!(!std::path::Path::new(&path).exists());
    let r = s.execute_checked("file.exportAep", json!({"path": path, "dryRun": true})).unwrap();
    assert_eq!(r["dryRun"], true);
    assert!(!r["report"]["losses"].as_array().unwrap().is_empty());
    assert!(!std::path::Path::new(&path).exists());
    let r = s.execute_checked("file.exportAep", json!({"path": path, "allowLosses": true})).unwrap();
    assert!(!r["report"]["losses"].as_array().unwrap().is_empty());
    assert!(std::path::Path::new(&path).exists());
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let mut s = session();
    assert!(s.execute_checked("file.exportAep", json!({})).is_err());
    assert!(s.execute_checked("file.exportAep", json!({"path": tmp("x.aep"), "bogus": 1})).is_err());
    assert!(s.execute_checked("file.exportAep", json!({"path": "/no/such/dir/at/all/x.aep"})).is_err());
    // An empty project exports too.
    let mut empty = Session::default();
    assert!(empty.execute_checked("file.exportAep", json!({"path": tmp("empty.aep")})).is_ok());
}

#[test]
fn aep_export_menu_entry_exists() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.exportAep"), Some(("File > Export".into(), "After Effects Project...".into())));
}
