//! File ▸ Import ▸ After Effects Project… through the command registry, with the synthetic
//! fixtures of `effectcraft-aep`.

use serde_json::json;

use crate::Session;

fn fixture(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name).to_string_lossy().to_string()
}

#[test]
fn import_aep_reads_the_xml_form_like_the_binary_one() {
    let mut s = Session::default();
    let x = s.execute_checked("file.importAep", json!({"path": fixture("058-key-opacity-2key.aepx")})).unwrap();
    let mut t = Session::default();
    let b = t.execute_checked("file.importAep", json!({"path": fixture("058-key-opacity-2key.aep")})).unwrap();
    assert_eq!(x["report"], b["report"]);
    assert_eq!(x["report"]["imported"]["keyframes"], 2);
    let folder = effectcraft_project::ItemId(x["folder"].as_u64().unwrap());
    assert_eq!(s.project.item(folder).unwrap().name, "058-key-opacity-2key.aepx");
    let (cx, cb) = (effectcraft_project::ItemId(x["comps"][0].as_u64().unwrap()), effectcraft_project::ItemId(b["comps"][0].as_u64().unwrap()));
    assert_eq!(s.project.comp(cx), t.project.comp(cb));
}

#[test]
fn import_aep_creates_the_project_items_and_is_undoable() {
    let mut s = Session::default();
    let r = s.execute_checked("file.importAep", json!({"path": fixture("055-key-position-3key-inserted-t2.5.aep")})).unwrap();
    assert_eq!(r["report"]["imported"]["compositions"], 1);
    assert_eq!(r["report"]["imported"]["keyframes"], 3);
    assert_eq!(r["report"]["losses"], json!([]));
    let cid = effectcraft_project::ItemId(r["comps"][0].as_u64().unwrap());
    // The wrapper folder is named after the file; the comp opens.
    let folder = effectcraft_project::ItemId(r["folder"].as_u64().unwrap());
    assert_eq!(s.project.item(folder).unwrap().name, "055-key-position-3key-inserted-t2.5.aep");
    assert_eq!(s.state.active_comp, Some(cid));
    let c = s.project.comp(cid).unwrap();
    assert_eq!((c.width, c.height, c.layers.len()), (1920, 1080, 1));
    let pos = c.layers[0].props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 3);
    assert_eq!(pos.value_at(effectcraft_time::Tick(effectcraft_time::TICKS_PER_SECOND)), effectcraft_keyframe::Value::Vec3([100.0, 0.0, 0.0]));

    // One undo step removes everything the import added; redo brings it back.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.items.is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.project.comp(cid).is_some());
}

#[test]
fn losses_are_refused_unless_allowed() {
    let mut s = Session::default();
    let path = fixture("077-fx-dropdown-default.aep");
    let err = s.execute_checked("file.importAep", json!({"path": path})).unwrap_err().to_string();
    assert!(err.contains("effects: 1") && err.contains("allowLosses"), "{err}");
    assert!(s.project.items.is_empty());
    assert!(s.history.undo.is_empty());

    // A dry run reports without importing.
    let r = s.execute_checked("file.importAep", json!({"path": path, "dryRun": true})).unwrap();
    assert_eq!(r["dryRun"], true);
    assert_eq!(r["report"]["losses"][0]["category"], "effects");
    assert_eq!(r["report"]["losses"][0]["count"], 1);
    assert!(s.project.items.is_empty());

    let r = s.execute_checked("file.importAep", json!({"path": path, "allowLosses": true})).unwrap();
    assert_eq!(r["report"]["losses"][0]["category"], "effects");
    assert!(!s.project.items.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.items.is_empty());
}

#[test]
fn bad_input_is_an_error_not_a_crash() {
    let mut s = Session::default();
    assert!(s.execute_checked("file.importAep", json!({})).is_err());
    assert!(s.execute_checked("file.importAep", json!({"path": "does/not/exist.aep"})).is_err());
    assert!(s.execute_checked("file.importAep", json!({"path": fixture("000-base.aep"), "bogus": 1})).is_err());
    let dir = std::env::temp_dir().join(format!("effectcraft-aep-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.aep");
    std::fs::write(&junk, b"this is not an aep").unwrap();
    let err = s.execute_checked("file.importAep", json!({"path": junk.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("RIFX"), "{err}");
    let cut = dir.join("cut.aep");
    let bytes = std::fs::read(fixture("000-base.aep")).unwrap();
    std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    assert!(s.execute_checked("file.importAep", json!({"path": cut.to_string_lossy(), "allowLosses": true})).is_err());
    assert!(s.project.items.is_empty());
}

#[test]
fn aep_menu_entry_exists() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.importAep"), Some(("File > Import".into(), "After Effects Project...".into())));
}

#[test]
fn import_aep_takes_the_project_settings_into_an_empty_project_only() {
    use effectcraft_project::{BitDepth, TimeDisplayStyle};
    // 621: the base with 8 bits per channel; 623: working space None; 629: frames.
    let mut s = Session::default();
    std::sync::Arc::make_mut(&mut s.project).settings.bit_depth = BitDepth::Bpc16;
    s.execute_checked("file.importAep", json!({"path": fixture("621-settings-bpc-8.aep")})).unwrap();
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc8);
    assert_eq!(s.project.settings.working_space, Some(effectcraft_color::ColorSpace::Srgb));
    // A second file into a project with compositions leaves the settings alone ...
    s.execute_checked("file.importAep", json!({"path": fixture("623-settings-workingspace-none.aep")})).unwrap();
    assert_eq!(s.project.settings.working_space, Some(effectcraft_color::ColorSpace::Srgb));
    // ... unless asked; and undo restores them.
    s.execute_checked("file.importAep", json!({"path": fixture("629-settings-time-display-frames.aep"), "applySettings": true})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Frames);
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc32);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.settings.time_display, TimeDisplayStyle::Timecode);
    assert_eq!(s.project.settings.bit_depth, BitDepth::Bpc8);
}
