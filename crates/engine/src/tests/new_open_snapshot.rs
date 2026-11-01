//! File ▸ New Project and Open keep a copy of the project they replace when it has unsaved
//! changes (ported from FilmCraft #3621): the copy is an auto-save version, its path is in the
//! command result, and a replacement that cannot keep one is refused unless `discard` is set.

use std::path::PathBuf;
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemKind, Project};
use serde_json::json;

use crate::Session;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ec-snapshot-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session whose auto-saves go to `dir/versions`, with a comp named `name` (so it is dirty).
fn dirty_session(dir: &std::path::Path, name: &str) -> Session {
    let mut s = Session { autosave_folder_override: Some(dir.join("versions")), ..Default::default() };
    s.execute("comp.new", json!({"name": name, "width": 64, "height": 64, "duration": 1})).unwrap();
    assert!(s.is_dirty());
    s
}

fn comp_names(p: &Project) -> Vec<String> {
    p.items.values().filter(|i| matches!(i.kind, ItemKind::Comp(_))).map(|i| i.name.clone()).collect()
}

#[test]
fn new_project_snapshots_a_dirty_untitled_project_first() {
    let d = tmp("new");
    let mut s = dirty_session(&d, "Precious");
    let r = s.execute("file.newProject", json!({})).unwrap();
    let snap = r["snapshot"].as_str().expect("the result names the snapshot").to_string();
    assert!(snap.starts_with(d.join("versions").to_str().unwrap()), "{snap}");
    let kept = Project::from_json(&std::fs::read_to_string(&snap).unwrap()).unwrap();
    assert_eq!(comp_names(&kept), ["Precious"], "the snapshot holds the replaced project");
    assert!(comp_names(&s.project).is_empty(), "the project was replaced");
}

#[test]
fn open_snapshots_a_dirty_project_first() {
    let d = tmp("open");
    let other = d.join("other.ecproj");
    {
        let mut o = Session::default();
        o.execute("file.saveAs", json!({"path": other.to_string_lossy()})).unwrap();
    }
    let mut s = dirty_session(&d, "Precious");
    let r = s.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap();
    let snap = r["snapshot"].as_str().expect("the result names the snapshot");
    let kept = Project::from_json(&std::fs::read_to_string(snap).unwrap()).unwrap();
    assert_eq!(comp_names(&kept), ["Precious"]);
    assert_eq!(s.path.as_deref(), other.to_str());
}

#[test]
fn a_clean_project_is_replaced_without_a_snapshot() {
    let d = tmp("clean");
    let mut s = Session { autosave_folder_override: Some(d.join("versions")), ..Default::default() };
    assert!(!s.is_dirty());
    let r = s.execute("file.newProject", json!({})).unwrap();
    assert!(r["snapshot"].is_null());
    assert!(!d.join("versions").exists(), "nothing was written");
}

#[test]
fn a_dirty_project_with_nowhere_to_keep_a_copy_is_refused_unless_discarded() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Precious", "width": 64, "height": 64, "duration": 1})).unwrap();
    let e = s.execute("file.newProject", json!({})).unwrap_err().to_string();
    assert!(e.contains("`discard: true`"), "{e}");
    assert_eq!(comp_names(&s.project), ["Precious"], "the project is kept");
    assert!(s.is_dirty());
    let other = tmp("refused").join("o.ecproj");
    Session::default().execute("file.saveAs", json!({"path": other.to_string_lossy()})).unwrap();
    let e = s.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("`discard: true`"), "{e}");
    assert_eq!(comp_names(&s.project), ["Precious"]);
    let r = s.execute("file.newProject", json!({"discard": true})).unwrap();
    assert!(r["snapshot"].is_null());
    assert!(comp_names(&s.project).is_empty());
}

#[test]
fn a_snapshot_keeps_document_relative_footage_pointing_at_the_original_file() {
    let d = tmp("relative");
    let mut s = dirty_session(&d, "Precious");
    let films = d.join("films");
    let doc = films.join("cut.ecproj").to_string_lossy().into_owned();
    let f = Footage { path: "media/clip.mp4".into(), kind: FootageKind::Video, has_video: true, ..Default::default() };
    let id = Arc::make_mut(&mut s.project).add_item("clip", Label::None, None, ItemKind::Footage(f));
    s.stored_paths.add(&s.project, Some(&doc));
    let snap = s.execute("file.newProject", json!({})).unwrap()["snapshot"].as_str().unwrap().to_string();
    let kept = Project::from_json(&std::fs::read_to_string(&snap).unwrap()).unwrap();
    let Some(ItemKind::Footage(f)) = kept.items.get(&id).map(|i| &i.kind) else { panic!("footage item") };
    assert_eq!(std::path::Path::new(&f.path), films.join("media/clip.mp4"), "the snapshot names the original file, not one beside the auto-save");
}

#[test]
fn successive_snapshots_rotate_through_the_versions() {
    let d = tmp("rotate");
    let mut s = dirty_session(&d, "First");
    s.autosave_now().unwrap(); // an ordinary auto-save established the slot
    let first = s.execute("file.newProject", json!({})).unwrap()["snapshot"].as_str().unwrap().to_string();
    s.execute("comp.new", json!({"name": "Second", "width": 64, "height": 64, "duration": 1})).unwrap();
    let second = s.execute("file.newProject", json!({})).unwrap()["snapshot"].as_str().unwrap().to_string();
    assert_ne!(first, second, "the second snapshot does not overwrite the first project's copy");
    let kept = Project::from_json(&std::fs::read_to_string(&first).unwrap()).unwrap();
    assert_eq!(comp_names(&kept), ["First"]);
}
