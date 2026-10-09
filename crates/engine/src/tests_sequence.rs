//! Image sequences (#297): File ▸ Import with the "<format> Sequence" option, picked ranges,
//! folders, Force Alphabetical Order, the import frame rate, Replace Footage, missing frames
//! (placeholders, as in After Effects) and Interpret Footage ▸ Start Frame.

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;

/// Probes every file as a 64×32 still (the media layer's single-file probe).
struct Stills;
impl crate::Importer for Stills {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("{path}: no such file"));
        }
        Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 64, height: 32, has_video: true, codec: "PNG".into(), ..Default::default() })
    }
}

/// A fresh folder with empty files of these names; returns its path.
fn folder(names: &[&str]) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("ec-seq-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for n in names {
        std::fs::write(d.join(n), b"").unwrap();
    }
    d.to_string_lossy().to_string()
}

fn at(dir: &str, name: &str) -> String {
    std::path::Path::new(dir).join(name).to_string_lossy().to_string()
}

fn session() -> Session {
    let mut s = Session { importer: Some(std::sync::Arc::new(Stills)), ..Default::default() };
    s.prefs.import.sequence_fps = 30.0;
    s
}

fn footage(s: &Session, item: &Value) -> (String, Footage) {
    let it = s.project.item(ItemId(item.as_u64().unwrap())).unwrap();
    let ItemKind::Footage(f) = &it.kind else { panic!("footage") };
    (it.name.clone(), f.clone())
}

const RUN: [&str; 5] = ["shot_0001.png", "shot_0002.png", "shot_0004.png", "shot_0005.png", "notes.txt"];

#[test]
fn one_picked_numbered_file_imports_its_run_as_one_sequence() {
    let dir = folder(&RUN);
    let mut s = session();
    let r = s.execute("file.import", json!({"paths": [at(&dir, "shot_0002.png")]})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 1, "{r}");
    let (name, f) = footage(&s, &r["items"][0]);
    assert_eq!(name, "shot_[0001-0005].png");
    assert_eq!((f.kind, f.sequence.len(), f.alphabetical), (FootageKind::Sequence, 4, false));
    assert_eq!(f.path, at(&dir, "shot_0001.png"));
    // Frames 1–5 at Settings ▸ Import ▸ Sequence Footage (30 fps); frame 3 is a placeholder.
    assert_eq!(f.duration, FrameRate::FPS_30.tick_of(5));
    assert_eq!(f.sequence_file(2), None);
    // Settings ▸ Import ▸ Report Missing Frames.
    let w = r["warnings"][0].as_str().unwrap();
    assert!(w.contains("1 missing frame (3)"), "{w}");
}

/// Picking every frame of a sequence used to make one item per frame (each a whole sequence on
/// the desktop, a still in the browser).
#[test]
fn picking_every_frame_imports_one_sequence() {
    let dir = folder(&RUN);
    let mut s = session();
    let all: Vec<String> = RUN[..4].iter().map(|n| at(&dir, n)).collect();
    let r = s.execute("file.import", json!({"paths": all, "frameRate": 24})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 1, "{r}");
    let (_, f) = footage(&s, &r["items"][0]);
    assert_eq!(f.sequence.len(), 4);
    // `frameRate` sets the sequence's rate on import.
    assert_eq!((f.frame_rate, f.duration), (FrameRate::FPS_24, FrameRate::FPS_24.tick_of(5)));
    // A picked range is just that range.
    let r = s.execute("file.import", json!({"paths": [at(&dir, "shot_0004.png"), at(&dir, "shot_0005.png")]})).unwrap();
    let (name, f) = footage(&s, &r["items"][0]);
    assert_eq!((name.as_str(), f.sequence.len()), ("shot_[0004-0005].png", 2));
    assert!(s.execute("file.import", json!({"paths": all, "frameRate": 0})).is_err());
}

#[test]
fn the_sequence_option_off_imports_stills() {
    let dir = folder(&RUN);
    let mut s = session();
    let r = s.execute("file.import", json!({"paths": [at(&dir, "shot_0002.png")], "sequence": false})).unwrap();
    let (name, f) = footage(&s, &r["items"][0]);
    assert_eq!((name.as_str(), f.kind), ("shot_0002.png", FootageKind::Still));
    let r = s.execute("file.import", json!({"paths": [at(&dir, "shot_0001.png"), at(&dir, "shot_0002.png")], "sequence": false})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 2);
}

#[test]
fn a_folder_imports_its_files_with_runs_as_sequences() {
    let dir = folder(&["a_01.png", "a_02.png", "b.png", "readme.txt"]);
    let mut s = session();
    let r = s.execute("file.import", json!({"paths": [dir], "background": true})).unwrap();
    s.execute("jobs.wait", json!({})).unwrap();
    assert!(r["job"].is_u64() || r["job"].is_string(), "{r}");
    let names: Vec<String> = s.project.items.values().map(|i| i.name.clone()).collect();
    assert_eq!(names, ["a_[01-02].png", "b.png"], "the text file is skipped");
}

#[test]
fn force_alphabetical_order_takes_every_image_of_the_type() {
    let dir = folder(&["take_b.png", "take_a.png", "other_7.png", "c.jpg"]);
    let mut s = session();
    let r = s.execute("file.import", json!({"paths": [at(&dir, "take_a.png")], "alphabetical": true})).unwrap();
    let (name, f) = footage(&s, &r["items"][0]);
    let files: Vec<String> = f.sequence.iter().map(|p| crate::sequence::file_name(p)).collect();
    assert_eq!(files, ["other_7.png", "take_a.png", "take_b.png"]);
    // Its numbering doesn't count: the files play one after another.
    assert_eq!((name.as_str(), f.alphabetical, f.sequence_frames()), ("other_7.png", true, 3));
    assert!(r["warnings"].as_array().is_none_or(Vec::is_empty), "{r}");
}

#[test]
fn interpret_footage_sets_start_frame() {
    let dir = folder(&RUN);
    let mut s = session();
    let item = s.execute("file.import", json!({"paths": [at(&dir, "shot_0001.png")]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 32, "frameRate": 30, "duration": 10})).unwrap();
    let layer = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let out = |s: &Session| s.active_comp().unwrap().layer(effectcraft_project::LayerId(layer)).unwrap().out_point;
    assert_eq!(out(&s), FrameRate::FPS_30.tick_of(5));
    // Start Frame 2 trims frame 1: 4 frames, and the layer that ran to the end still does.
    s.execute("file.interpretFootage", json!({"items": [item], "startFrame": 2})).unwrap();
    assert_eq!((footage(&s, &item).1.duration, out(&s)), (FrameRate::FPS_30.tick_of(4), FrameRate::FPS_30.tick_of(4)));
    // Start Frame 0: frame 0 is missing (a placeholder), 6 frames in all.
    let r = s.execute("file.interpretFootage", json!({"items": [item], "startFrame": 0})).unwrap();
    assert_eq!((r["items"][0]["startFrame"].as_i64(), r["items"][0]["frames"].as_i64()), (Some(0), Some(6)));
    assert_eq!(footage(&s, &item).1.sequence_file(0), None);
    // The first file's own number isn't stored; "file" goes back to it.
    s.execute("file.interpretFootage", json!({"items": [item], "startFrame": 1})).unwrap();
    assert_eq!(footage(&s, &item).1.start_frame, None);
    s.execute("file.interpretFootage", json!({"items": [item], "startFrame": 3})).unwrap();
    s.execute("file.interpretFootage", json!({"items": [item], "startFrame": "file"})).unwrap();
    assert_eq!(footage(&s, &item).1.start_frame, None);
    // The frame rate re-times the frames.
    s.execute("file.interpretFootage", json!({"items": [item], "frameRate": 10})).unwrap();
    assert_eq!(footage(&s, &item).1.duration, Tick::from_seconds_f64(0.5));
    for bad in [json!({"startFrame": "x"}), json!({"startFrame": 1e300})] {
        let mut p = bad.clone();
        p["items"] = json!([item]);
        assert!(s.execute("file.interpretFootage", p).is_err(), "{bad}");
    }
    // Undo puts the interpretation back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(footage(&s, &item).1.frame_rate, FrameRate::FPS_30);
}

#[test]
fn replace_footage_and_reload_keep_sequences() {
    let dir = folder(&RUN);
    let mut s = session();
    let item = s.execute("file.import", json!({"paths": [at(&dir, "shot_0001.png")], "sequence": false})).unwrap()["items"][0].clone();
    s.state.project_selection = vec![ItemId(item.as_u64().unwrap())];
    s.execute("file.replaceFootage", json!({"path": at(&dir, "shot_0004.png")})).unwrap();
    let (name, f) = footage(&s, &item);
    assert_eq!((name.as_str(), f.sequence.len()), ("shot_[0001-0005].png", 4));
    // Reload finds a new frame and keeps the interpretation.
    s.execute("file.interpretFootage", json!({"items": [item], "frameRate": 12, "startFrame": 2})).unwrap();
    std::fs::write(at(&dir, "shot_0003.png"), b"").unwrap();
    s.execute("file.reloadFootage", json!({"items": [item]})).unwrap();
    let (_, f) = footage(&s, &item);
    assert_eq!((f.sequence.len(), f.frame_rate, f.start_frame), (5, FrameRate::from_f64(12.0), Some(2)));
    assert_eq!(f.duration, FrameRate::from_f64(12.0).tick_of(4));
}
