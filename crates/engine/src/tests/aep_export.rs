//! File ▸ Export ▸ Adobe After Effects Project… through the command registry.

use serde_json::json;

use crate::Session;

/// A path `name` in a folder of its own that is removed with the returned guard.
fn tmp(name: &str) -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join(name).to_string_lossy().to_string();
    (d, path)
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
    let (_dir, path) = tmp("main.aep");
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
    let (_dir, path) = tmp("lossy.aep");
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
    let (_dir, x) = tmp("x.aep");
    assert!(s.execute_checked("file.exportAep", json!({})).is_err());
    assert!(s.execute_checked("file.exportAep", json!({"path": x, "bogus": 1})).is_err());
    assert!(s.execute_checked("file.exportAep", json!({"path": "/no/such/dir/at/all/x.aep"})).is_err());
    // An empty project exports too.
    let mut empty = Session::default();
    let (_empty_dir, path) = tmp("empty.aep");
    assert!(empty.execute_checked("file.exportAep", json!({"path": path})).is_ok());
}

/// File ▸ Export ▸ Adobe After Effects Project… is in the menu under that label.
#[test]
fn aep_export_menu_entry_exists() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.exportAep"), Some(("File > Export".into(), "Adobe After Effects Project...".into())));
}

#[test]
fn a_project_made_with_commands_exports_without_false_losses() {
    // The export reports values it changes. What the commands make by default (effects added at
    // the layer's bounds, masks with the cycled colours, also after one is removed) is not such a
    // value.
    let mut s = session();
    for effect in ["ec.blur.fastbox", "ec.time.timewarp", "ec.color.tint", "ec.perspective.dropshadow"] {
        s.execute_checked("effect.apply", json!({"layers": ["#1"], "effect": effect})).unwrap();
    }
    for _ in 0..3 {
        s.execute_checked("mask.new", json!({"layer": "#1", "vertices": [[0, 0], [50, 0], [50, 40]]})).unwrap();
    }
    s.execute_checked("mask.remove", json!({"layer": "#1", "mask": "Mask 1"})).unwrap();
    let (_dir, path) = tmp("commands.aep");
    let r = s.execute_checked("file.exportAep", json!({"path": path, "dryRun": true})).unwrap();
    assert_eq!(r["report"]["losses"], json!([]));
    assert_eq!(r["report"]["exported"]["effects"], 4);
    assert_eq!(r["report"]["exported"]["masks"], 2);
}

/// File access that records the folders it is asked the time of and gives each the same time.
#[derive(Default)]
struct Stamps(std::sync::Mutex<Vec<String>>);

/// The time `Stamps` gives: 2026-10-19 03:51:22 UTC, stored as 0xe6fb443a (seconds since 1904).
const FOLDER_SECONDS: u64 = 1_792_381_882;

impl crate::Services for Stamps {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }
    fn stamp(&self, path: &str) -> Option<(u64, u128)> {
        self.0.lock().unwrap().push(path.to_string());
        Some((0, u128::from(FOLDER_SECONDS) * 1_000_000_000))
    }
}

/// The stored folder time (`sspc@118`) of the sequence item of an exported project.
fn sequence_folder_time(bytes: &[u8]) -> u32 {
    let file = effectcraft_aep::parse(bytes).unwrap();
    let view = effectcraft_aep::Project::new(&file).unwrap();
    let item = view.all_items().into_iter().find(|i| i.footage().is_some_and(|f| f.is_sequence)).expect("sequence");
    let sspc = item.chunk().child_list(b"Pin ").and_then(|pin| pin.child(b"sspc")).and_then(|c| c.bytes()).expect("sspc");
    u32::from_be_bytes(sspc[118..122].try_into().unwrap())
}

/// A session whose project has one png sequence of three frames in `dir` (as `sep` joins them),
/// recorded as stored in the document `document`.
fn with_sequence(dir: &str, sep: &str, document: &str) -> (Session, std::sync::Arc<Stamps>) {
    use effectcraft_project::{Footage, FootageKind, ItemKind};
    let stamps = std::sync::Arc::new(Stamps::default());
    let mut s = Session { services: stamps.clone(), ..Default::default() };
    let files: Vec<String> = (1..=3).map(|k| format!("{dir}{sep}a_{k:04}.png")).collect();
    let mut f = Footage {
        path: files[0].clone(),
        kind: FootageKind::Sequence,
        frame_rate: effectcraft_time::FrameRate::new(25, 1),
        sequence: files,
        width: 64,
        height: 48,
        has_video: true,
        ..Default::default()
    };
    f.sync_sequence_duration();
    std::sync::Arc::make_mut(&mut s.project).add_item("a_[0001-0003].png", Default::default(), None, ItemKind::Footage(f));
    s.stored_paths.add(&s.project, Some(document));
    (s, stamps)
}

#[test]
fn a_sequence_folder_is_looked_at_only_through_the_document_path_policy() {
    let root = std::env::temp_dir().join(format!("ec-aep-export-folder-time-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("seq")).unwrap();
    let document = root.join("job.aep").to_string_lossy().into_owned();
    let out = root.join("out.aep").to_string_lossy().into_owned();
    // A sequence on another computer, stored in an imported project: refused by the policy, so its
    // folder is never asked about and the time stays unset (0).
    let (mut s, stamps) = with_sequence(r"\\192.0.2.1\share\seq", r"\", &document);
    let first = r"\\192.0.2.1\share\seq\a_0001.png";
    assert!(matches!(s.stored_path(first), Err(crate::doc_path::NotFollowed::Refused(_))));
    s.execute_checked("file.exportAep", json!({"path": out, "allowLosses": true})).unwrap();
    assert_eq!(*stamps.0.lock().unwrap(), Vec::<String>::new());
    assert_eq!(sequence_folder_time(&std::fs::read(&out).unwrap()), 0);
    // Chosen in this session (imported or relinked by the user), the same path is followed as
    // given, like every other footage access.
    let f = s.project.items.values().find_map(|i| match &i.kind {
        effectcraft_project::ItemKind::Footage(f) => Some(f.clone()),
        _ => None,
    });
    s.stored_paths.allow(&f.unwrap());
    s.execute_checked("file.exportAep", json!({"path": out, "allowLosses": true})).unwrap();
    assert_eq!(stamps.0.lock().unwrap().len(), 1);
    // A local sequence beside the document: its folder's time is written.
    let local = root.join("seq").to_string_lossy().into_owned();
    let (mut s, stamps) = with_sequence(&local, std::path::MAIN_SEPARATOR_STR, &document);
    s.execute_checked("file.exportAep", json!({"path": out, "allowLosses": true})).unwrap();
    assert_eq!(*stamps.0.lock().unwrap(), [local]);
    assert_eq!(sequence_folder_time(&std::fs::read(&out).unwrap()), 0xe6fb_443a);
    let _ = std::fs::remove_dir_all(&root);
}
